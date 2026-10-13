//! Native-only community IPC. Registered sources, owned children, no renderer networking.
use crate::workspace;
use demodesk_core::{engine::Engine, render::process::ProcessTree};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Command, Stdio},
    sync::{atomic::{AtomicBool, Ordering}, Arc, Mutex},
    time::{Duration, Instant},
};

pub struct Community {
    engine: Arc<Engine>,
    cached: Mutex<Value>,
    activity: Mutex<Value>,
    current: Mutex<Option<Arc<AtomicBool>>>,
    stopped: AtomicBool,
}

impl Community {
    pub fn new(engine: Arc<Engine>) -> Arc<Self> {
        let this = Arc::new(Self {
            engine, cached: Mutex::new(json!({"schemaVersion":1,
                "settings":{"endpoint":"https://157.173.114.38:9443","sharingEnabled":false,"speechEnabled":false},
                "feedback":[],"contributions":[],"operator":{"configured":false},
                "training":{"state":"needs_examples","verifiedExamples":0,"versions":[]}})),
            activity: Mutex::new(json!({"busy":false})),
            current: Mutex::new(None), stopped: AtomicBool::new(false),
        });
        let initial = this.clone();
        std::thread::spawn(move || { let _ = initial.refresh(); });
        this
    }

    pub fn snapshot(&self) -> Value {
        let mut snapshot = self.cached.lock().unwrap().clone();
        snapshot["activity"] = self.activity.lock().unwrap().clone();
        snapshot
    }

    fn refresh(&self) -> Result<Value, String> {
        let value = self.call("status", json!({}), Arc::new(AtomicBool::new(false)), false)?;
        *self.cached.lock().unwrap() = value;
        Ok(self.snapshot())
    }

    pub fn profile(&self, player: &str) -> Result<Value, String> {
        self.call("profile_snapshot", json!({"playerId":player}), Arc::new(AtomicBool::new(false)), false)
    }

    fn registered_source(&self, values: &mut Value) -> Result<(), String> {
        let consent = &values["consent"];
        for key in ["training", "rightsConfirmed", "personalDataAcknowledged"] {
            if consent[key] != true {
                return Err("Confirm the contribution purpose and contents before preparing it.".into());
            }
        }
        let source = match values["kind"].as_str() {
            Some("demo") => {
                let id = values["demoId"].as_str().ok_or("Choose a registered match.")?;
                let (meta, parsed) = self.engine.get_demo(id).ok_or("This match is unavailable.")?;
                if let Some(parsed) = parsed { values["_sourceMatchHash"] = json!(parsed.demo_digest); }
                std::path::PathBuf::from(meta.path)
            }
            Some("example") => {
                let id = values["jobId"].as_str().ok_or("Choose a registered clip.")?.to_string();
                let key = values["outputKey"].as_str().ok_or("Choose a registered output.")?.to_string();
                let jobs = self.engine.list_jobs().map_err(|e| e.to_string())?;
                let job = jobs.iter().find(|job| job.id == id).ok_or("This export is unavailable.")?;
                let output = job.outputs.iter().find(|output| output.file == key)
                    .ok_or("This video is not a registered output of this export.")?;
                if let Some(snapshot) = &job.source_snapshot {
                    if let Some(digest) = snapshot.fingerprint.strip_prefix("sha256:") {
                        values["_sourceMatchHash"] = json!(digest);
                    }
                }
                self.engine.get_clip_media(&id, &key).map_err(|e| e.to_string())?;
                std::path::PathBuf::from(&output.file)
            }
            _ => return Err("Choose a demo or clip contribution.".into()),
        };
        let source = workspace::owned_path(&source)?;
        if !source.is_file() { return Err("The contribution source is missing.".into()); }
        values["_sourcePath"] = json!(source);
        if values["kind"] == "example" {
            let ffmpeg = self.engine.tool_paths().ffmpeg_exe.ok_or("Install FFmpeg before preparing a clip.")?;
            values["_ffmpegPath"] = json!(workspace::owned_path(&ffmpeg)?);
        }
        Ok(())
    }

    fn validate_feedback(&self, values: &mut Value) -> Result<(), String> {
        if values["sourceKind"]=="clip" || values["demoId"].is_null() {
            let id = values["jobId"].as_str().ok_or("Choose a registered export.")?.to_string();
            let key = values["outputKey"].as_str().ok_or("Choose a registered clip output.")?.to_string();
            let jobs = self.engine.list_jobs().map_err(|e|e.to_string())?;
            let job = jobs.iter().find(|job|job.id==id).ok_or("Choose a registered export.")?;
            let output = job.outputs.iter().find(|out|out.file==key).ok_or("Choose a registered clip output.")?;
            if let Some(snapshot) = &job.source_snapshot {
                values["sourceFingerprint"] = json!(snapshot.fingerprint);
                if let Some(highlight) = snapshot.highlights.iter().find(|h|Some(h.id.as_str())==output.highlight_id.as_deref()) {
                    values["originalProposal"] = json!({"title":highlight.title,"tags":highlight.tags,
                        "startTick":highlight.start_tick,"endTick":highlight.end_tick,"source":"registered-export"});
                    values["playerId"] = json!(highlight.player.steamid);
                    validate_feedback_ticks(values, highlight.end_tick.max(highlight.start_tick))?;
                }
            } else {
                values.as_object_mut().unwrap().remove("sourceFingerprint");
            }
        } else if let Some(id) = values["demoId"].as_str() {
            let (_, parsed) = self.engine.get_demo(id).ok_or("Choose a registered match.")?;
            let parsed = parsed.ok_or("Parse the match before saving timeline feedback.")?;
            if let Some(player) = values["playerId"].as_str() {
                if !parsed.info.players.iter().any(|p| p.steamid == player) {
                    return Err("Choose a player from this parsed match.".into());
                }
            }
            validate_feedback_ticks(values, parsed.demo_end_tick)?;
            values["sourceFingerprint"] = json!(parsed.demo_digest);
            if let Some(moment_id) = values["momentId"].as_str() {
                if let Some(candidate) = parsed.discovery_candidates.iter().find(|candidate|candidate.id==moment_id) {
                    values["originalProposal"] = json!({"title":candidate.title,"tags":candidate.tags,
                        "startTick":candidate.start_tick,"endTick":candidate.end_tick,"source":"native-discovery"});
                } else if let Some(highlight) = parsed.highlights.iter().find(|highlight|highlight.id==moment_id) {
                    values["originalProposal"] = json!({"title":highlight.title,"tags":highlight.tags,
                        "startTick":highlight.start_tick,"endTick":highlight.end_tick,"source":"native-highlight"});
                }
            }
        } else {
            return Err("Feedback must belong to a registered match or clip.".into());
        }
        Ok(())
    }

    pub fn action(self: &Arc<Self>, action: &str, mut values: Value) -> Result<Value, String> {
        if !values.is_object() { return Err("Community input must be an object.".into()); }
        if action == "status" {
            if values["refresh"] == true && self.current.lock().unwrap().is_none() {
                return self.refresh();
            }
            return Ok(self.snapshot());
        }
        if matches!(action, "contribution_pause"|"contribution_cancel"|"cancel") {
            if let Some(flag) = &*self.current.lock().unwrap() {
                flag.store(true, Ordering::SeqCst);
                self.activity.lock().unwrap()["cancelling"] = json!(true);
                return Ok(self.snapshot());
            }
        }
        if action == "feedback_save" { self.validate_feedback(&mut values)?; }
        if action == "contribution_prepare" { self.registered_source(&mut values)?; }
        let background = matches!(action, "contribution_prepare"|"contribution_upload"|"contribution_retry"
            |"teacher_review"|"train"|"evaluate");
        if !background {
            let value = self.call(action, values, Arc::new(AtomicBool::new(false)), false)?;
            self.refresh()?;
            return Ok(value);
        }
        let mut current = self.current.lock().unwrap();
        if current.is_some() { return Err("Finish or cancel the current community task first.".into()); }
        let cancel = Arc::new(AtomicBool::new(false));
        *current = Some(cancel.clone());
        *self.activity.lock().unwrap() = json!({"busy":true,"task":action,"phase":"starting",
            "progress":"Starting…","startedAt":demodesk_core::store::now(),"cancelling":false,
            "contributionId":values["id"]});
        let state = self.clone();
        let action = action.to_string();
        std::thread::spawn(move || {
            let outcome = state.call(&action, values.clone(), cancel.clone(), true);
            if cancel.load(Ordering::SeqCst) && values["id"].is_string() {
                let _ = state.call("contribution_pause", values, Arc::new(AtomicBool::new(false)), false);
            }
            *state.activity.lock().unwrap() = match outcome {
                Ok(result) => json!({"busy":false,"progress":"Finished","result":result}),
                Err(error) => json!({"busy":false,"error":error,"progress":if cancel.load(Ordering::SeqCst) {"Cancelled"} else {"Needs attention"}}),
            };
            *state.current.lock().unwrap() = None;
            let _ = state.refresh();
        });
        drop(current);
        Ok(self.snapshot())
    }

    pub fn shutdown(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        if let Some(cancel) = &*self.current.lock().unwrap() { cancel.store(true, Ordering::SeqCst); }
    }

    fn call(&self, action: &str, values: Value, cancel: Arc<AtomicBool>, progress: bool) -> Result<Value,String> {
        let source = serde_json::to_vec(&json!({"command":"community","action":action,"values":values}))
            .map_err(|_|"Invalid community request")?;
        if source.len()>1048576 { return Err("Community input exceeds its safety limit.".into()); }
        let tree = ProcessTree::new().map_err(|_|"Cannot create the community process group.")?;
        let python = workspace::root().join("tools/python-env/Scripts/python.exe");
        let portable = workspace::root().join("tools/python/python.exe");
        let mut command = Command::new(if python.is_file() {python} else {portable});
        command.arg("-u").arg(workspace::root().join("app/automation/cli.py"))
            .current_dir(workspace::root().join("app/automation"))
            .env("PYTHONNOUSERSITE", "1")
            .env("PYTHONPATH", "")
            .env_remove("PYTHONHOME")
            .env("CLIPFORGE_WORKSPACE",workspace::root())
            .env("PYTHONPYCACHEPREFIX",workspace::root().join("cache/pycache"))
            .env("HF_HOME",workspace::root().join("models/huggingface"))
            .env("XDG_CACHE_HOME",workspace::root().join("cache/python"))
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)] { use std::os::windows::process::CommandExt; command.creation_flags(0x08000000); }
        let mut child = command.spawn().map_err(|_|"Install the local Python helper before using community features.")?;
        if tree.attach_child(&child).is_err() {
            let _=child.kill();let _=child.wait();
            return Err("Cannot isolate the community helper.".into());
        }
        child.stdin.take().ok_or("Community input unavailable")?.write_all(&source).map_err(|_|"Cannot send community input")?;
        let stdout=child.stdout.take().ok_or("Community output unavailable")?;
        let output=std::thread::spawn(move|| {
            let mut bytes=Vec::new();stdout.take(4*1024*1024+1).read_to_end(&mut bytes).map(|_|bytes)
        });
        let stderr=child.stderr.take().ok_or("Community progress unavailable")?;
        let outcome=std::thread::scope(|scope| {
            scope.spawn(|| {
                let mut reader=BufReader::new(stderr);
                loop {
                    let mut line=String::new();
                    let size=Read::by_ref(&mut reader).take(8193).read_line(&mut line).unwrap_or(0);
                    if size==0 {break}
                    if size>8192 {continue}
                    if progress {
                        if let Ok(value)=serde_json::from_str::<Value>(&line) {
                            let mut state=self.activity.lock().unwrap();
                            if state["cancelling"]!=true {
                                for key in ["progress","phase","completed","total"] {
                                    if !value[key].is_null() {state[key]=value[key].clone();}
                                }
                            }
                        }
                    }
                }
            });
            let deadline=Instant::now()+Duration::from_secs(if progress {12*3600} else {40});
            loop {
                if cancel.load(Ordering::SeqCst)||self.stopped.load(Ordering::SeqCst)||Instant::now()>deadline {
                    let _=tree.finish();let _=child.wait();
                    return Err(if cancel.load(Ordering::SeqCst) {"The community task was cancelled."} else {"The community task timed out."}.to_string());
                }
                match child.try_wait() {
                    Ok(Some(status)) => {let _=tree.finish(); if !status.success() {return Err("The community helper stopped unexpectedly.".into())} break;},
                    Ok(None)=>std::thread::sleep(Duration::from_millis(100)),
                    Err(_)=>{let _=tree.finish();return Err("Cannot read community task status.".into())},
                }
            }
            Ok(())
        });
        let bytes=output.join().map_err(|_|"Cannot finish community results")?.map_err(|_|"Cannot read community results")?;
        outcome?;
        if bytes.len()>4*1024*1024 {return Err("Community results exceed their safety limit.".into())}
        let response:Value=serde_json::from_slice(&bytes).map_err(|_|"Invalid community results")?;
        if response["ok"]!=true {
            return Err(response["error"].as_str().unwrap_or("The community operation failed.").into());
        }
        Ok(response["result"].clone())
    }
}

fn validate_feedback_ticks(values:&Value, demo_end:i32) -> Result<(),String> {
    let start=values["startTick"].as_i64();
    let end=values["endTick"].as_i64();
    if start.is_some() || end.is_some() {
        let (Some(start),Some(end))=(start,end) else {return Err("Choose both ends of the feedback interval.".into())};
        if start<0 || end<=start || end>i64::from(demo_end) {
            return Err("The feedback interval must belong to this recorded source.".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn correction_cannot_leave_registered_source_or_be_partly_defined() {
        assert!(validate_feedback_ticks(&json!({"startTick":5,"endTick":10}),20).is_ok());
        for value in [json!({"startTick":-1,"endTick":10}),json!({"startTick":5}),
            json!({"startTick":20,"endTick":21}),json!({"startTick":10,"endTick":10})] {
            assert!(validate_feedback_ticks(&value,20).is_err());
        }
    }
}
