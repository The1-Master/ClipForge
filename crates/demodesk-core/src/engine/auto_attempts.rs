//! Durable, quality-independent automatic attempts. Kept outside clips so clearing
//! videos cannot silently enqueue the entire historical library again.
use crate::model::Highlight;
use crate::stats::ParsedDemo;
use crate::store::{RenderJob, SourceSnapshot};
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::Path;

const FILE: &str = "automatic-attempts.json";
const MAX_BYTES: u64 = 32 * 1024 * 1024;
const MAX_ATTEMPTS: usize = 200_000;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum Attempt {
    Observed {
        demo_id: String,
        source_fingerprint: String,
        player_id: String,
        moment_id: String,
        start_tick: i32,
        end_tick: i32,
    },
    // Old ordinary jobs have no source/player snapshot. Retain their actual
    // known identity, without manufacturing new metadata.
    Legacy {
        demo_id: String,
        moment_id: String,
    },
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Ledger {
    schema_version: u8,
    attempts: BTreeSet<Attempt>,
    source_aliases: BTreeMap<String, String>,
}
impl Default for Ledger {
    fn default() -> Self {
        Self {
            schema_version: 1,
            attempts: BTreeSet::new(),
            source_aliases: BTreeMap::new(),
        }
    }
}
fn bounded(value: &str, maximum: usize) -> bool {
    !value.is_empty() && value.len() <= maximum && !value.chars().any(char::is_control)
}
fn fingerprint(value: &str, kind: &str, length: usize) -> bool {
    value.strip_prefix(kind).is_some_and(|hash| {
        hash.len() == length
            && hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}
fn source(value: &str) -> bool {
    fingerprint(value, "sha256:", 64) || fingerprint(value, "sha1:", 40)
}

/// Only automatic scheduling consults this ledger. A model can move its clip
/// boundaries without causing another export when most of the requested scene
/// was already attempted. A substantially longer scene containing new material
/// remains eligible; manual exports never consult this policy.
fn already_covered(start: i32, end: i32, previous_start: i32, previous_end: i32) -> bool {
    let duration = i64::from(end) - i64::from(start);
    if duration <= 0 { return false; }
    let overlap = i64::from(end.min(previous_end)) - i64::from(start.max(previous_start));
    overlap > 0 && overlap * 5 >= duration * 3 // At least 60% of the new requested interval.
}
fn validate_file(root: &Path) -> Result<()> {
    let path = root.join(FILE);
    match std::fs::symlink_metadata(&path) {
        Ok(meta) => anyhow::ensure!(
            meta.is_file()
                && !meta.file_type().is_symlink()
                && path.canonicalize()?.parent() == Some(root.canonicalize()?.as_path()),
            "The automatic-attempt history path is unsafe."
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}
fn load(root: &Path) -> Result<Ledger> {
    validate_file(root)?;
    let path = root.join(FILE);
    let bytes = match std::fs::metadata(&path) {
        Ok(meta) => {
            anyhow::ensure!(
                meta.len() <= MAX_BYTES,
                "The automatic-attempt history exceeds its safe size."
            );
            let file = std::fs::File::open(&path)?;
            let mut bytes = Vec::new();
            file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
            anyhow::ensure!(
                bytes.len() as u64 <= MAX_BYTES,
                "The automatic-attempt history exceeds its safe size."
            );
            bytes
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Ledger::default()),
        Err(error) => return Err(error.into()),
    };
    let ledger:Ledger=serde_json::from_slice(&bytes).context("The automatic-attempt history is corrupt; automatic exports are paused. Manual export and playback are still available.")?;
    ledger.validate()?;
    Ok(ledger)
}
impl Ledger {
    fn validate(&self) -> Result<()> {
        anyhow::ensure!(self.schema_version==1 && self.attempts.len()<=MAX_ATTEMPTS && self.source_aliases.len()<=MAX_ATTEMPTS,"The automatic-attempt history has unsupported or excessive data; automatic exports are paused.");
        for attempt in &self.attempts {
            match attempt {
                Attempt::Observed{demo_id,source_fingerprint,player_id,moment_id,start_tick,end_tick}=>{
                    anyhow::ensure!(bounded(demo_id,128) && source(source_fingerprint) && bounded(player_id,64) && bounded(moment_id,512) && *start_tick>=0 && end_tick>start_tick,"The automatic-attempt history contains an invalid moment; automatic exports are paused.");
                },
                Attempt::Legacy{demo_id,moment_id}=>anyhow::ensure!(bounded(demo_id,128)&&bounded(moment_id,512),"The automatic-attempt history contains an invalid legacy marker; automatic exports are paused."),
            }
        }
        anyhow::ensure!(self.source_aliases.iter().all(|(old,new)|fingerprint(old,"sha1:",40)&&fingerprint(new,"sha256:",64)),"The automatic-attempt history contains an invalid source alias; automatic exports are paused.");
        Ok(())
    }
    fn save(&self, root: &Path) -> Result<()> {
        self.validate()?;
        validate_file(root)?;
        let bytes = serde_json::to_vec(self)?;
        anyhow::ensure!(
            bytes.len() as u64 <= MAX_BYTES,
            "The automatic-attempt history exceeds its safe size."
        );
        let mut staged = tempfile::Builder::new()
            .prefix("automatic-attempts-")
            .suffix(".tmp")
            .tempfile_in(root)?;
        staged.write_all(&bytes)?;
        staged.as_file().sync_all()?;
        staged.persist(root.join(FILE)).map_err(|error|anyhow!("The automatic-attempt history could not be saved atomically. Automatic export was not queued: {}",error.error))?;
        Ok(())
    }
    fn add_highlight(
        &mut self,
        demo_id: &str,
        source_fingerprint: &str,
        highlight: &Highlight,
    ) -> bool {
        if highlight.tags.iter().any(|tag| tag == "ai-preview") {
            return false;
        }
        self.attempts.insert(Attempt::Observed {
            demo_id: demo_id.into(),
            source_fingerprint: source_fingerprint.into(),
            player_id: highlight.player.steamid.clone(),
            moment_id: highlight.id.clone(),
            start_tick: highlight.start_tick,
            end_tick: highlight.end_tick,
        })
    }
    fn add_job(&mut self, job: &RenderJob) -> bool {
        if job
            .analysis_clips
            .as_ref()
            .is_some_and(|clips| clips.rule_id == "ai-preview")
        {
            return false;
        }
        let mut changed = false;
        if let Some(snapshot) = &job.source_snapshot {
            for highlight in &snapshot.highlights {
                changed |= self.add_highlight(&job.demo_id, &snapshot.fingerprint, highlight);
            }
        } else if let Some(clips) = &job.analysis_clips {
            for highlight in &clips.highlights {
                changed |= self.add_highlight(&job.demo_id, &clips.demo_fingerprint, highlight);
            }
        } else {
            for id in &job.highlight_ids {
                changed |= self.attempts.insert(Attempt::Legacy {
                    demo_id: job.demo_id.clone(),
                    moment_id: id.clone(),
                });
            }
        }
        changed
    }
    pub(super) fn reserve(
        &mut self,
        root: &Path,
        demo_id: &str,
        snapshot: &SourceSnapshot,
    ) -> Result<()> {
        for highlight in &snapshot.highlights {
            self.add_highlight(demo_id, &snapshot.fingerprint, highlight);
        }
        self.save(root).context("Cannot preserve this automatic attempt. Automatic export was not queued; manual export remains available.")
    }
    fn same_source(&self, old: &str, current: &str) -> bool {
        old == current
            || self
                .source_aliases
                .get(old)
                .is_some_and(|alias| alias == current)
    }
    pub(super) fn contains_highlight(
        &self,
        demo_id: &str,
        current: &str,
        highlight: &Highlight,
    ) -> bool {
        self.attempts.iter().any(|attempt| match attempt {
            Attempt::Observed {
                source_fingerprint,
                player_id,
                moment_id,
                start_tick,
                end_tick,
                ..
            } => {
                self.same_source(source_fingerprint, current)
                    && player_id == &highlight.player.steamid
                    && (moment_id == &highlight.id
                        || already_covered(highlight.start_tick, highlight.end_tick, *start_tick, *end_tick))
            }
            Attempt::Legacy {
                demo_id: old,
                moment_id,
            } => old == demo_id && moment_id == &highlight.id,
        })
    }
    pub(super) fn contains_window(
        &self,
        demo_id: &str,
        current: &str,
        player: &str,
        window: &[i32; 2],
        known: &[Highlight],
    ) -> bool {
        self.attempts.iter().any(|attempt| match attempt {
            Attempt::Observed {
                source_fingerprint,
                player_id,
                start_tick,
                end_tick,
                ..
            } => {
                self.same_source(source_fingerprint, current)
                    && player_id == player
                    && already_covered(window[0], window[1], *start_tick, *end_tick)
            }
            Attempt::Legacy {
                demo_id: old,
                moment_id,
            } => {
                old == demo_id
                    && known.iter().any(|h| {
                        h.id == *moment_id
                            && h.player.steamid == player
                            && already_covered(window[0], window[1], h.start_tick, h.end_tick)
                    })
            }
        })
    }
    pub(super) fn resolve_legacy_source(
        &mut self,
        root: &Path,
        demo_id: &str,
        path: &Path,
        parsed: &ParsedDemo,
    ) -> Result<()> {
        let unresolved: BTreeSet<_> = self
            .attempts
            .iter()
            .filter_map(|attempt| match attempt {
                Attempt::Observed {
                    demo_id: old,
                    source_fingerprint,
                    ..
                } if old == demo_id
                    && source_fingerprint.starts_with("sha1:")
                    && !self.source_aliases.contains_key(source_fingerprint) =>
                {
                    Some(source_fingerprint.clone())
                }
                _ => None,
            })
            .collect();
        if unresolved.is_empty() {
            return Ok(());
        }
        use sha2::{Digest, Sha256};
        let mut file = std::fs::File::open(path)?;
        let mut sha1 = sha1_smol::Sha1::new();
        let mut sha256 = Sha256::new();
        let mut buffer = [0; 65536];
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            sha1.update(&buffer[..count]);
            sha256.update(&buffer[..count]);
        }
        let current = format!("sha256:{:x}", sha256.finalize());
        anyhow::ensure!(
            current == format!("sha256:{}", parsed.demo_digest),
            "The automatic source changed; parse the match again before automatic export."
        );
        let old = format!("sha1:{}", sha1.digest());
        if unresolved.contains(&old) {
            self.source_aliases.insert(old, current);
            self.save(root)?;
        }
        Ok(())
    }
}
pub(super) fn migrate(root: &Path, jobs: &[RenderJob]) -> Result<Ledger> {
    let mut ledger = load(root)?;
    let changed = jobs
        .iter()
        .fold(false, |changed, job| ledger.add_job(job) || changed);
    if changed {
        ledger.save(root)?;
    }
    Ok(ledger)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn model_boundary_drift_and_new_individual_shots_do_not_repeat_already_covered_automatic_exports() {
        let source=format!("sha256:{}","a".repeat(64));
        let mut ledger=Ledger::default();
        let mut highlight:Highlight=serde_json::from_value(serde_json::json!({"id":"old-model-moment","player":{"steamid":"player","name":"Fixture"},"round":1,"startTick":100,"endTick":300,"anchorTick":150,"score":1,"tags":[],"title":"Original model label","kills":[],"breakdown":{}})).unwrap();
        ledger.add_highlight("demo",&source,&highlight);
        highlight.id="new-model-moment".into();highlight.title="New model description".into();highlight.start_tick=110;highlight.end_tick=310;
        assert!(ledger.contains_highlight("renamed-demo",&source,&highlight));
        assert!(ledger.contains_window("demo",&source,"player",&[110,310],&[]));
        assert!(ledger.contains_window("demo",&source,"player",&[140,180],&[]));
        assert!(!ledger.contains_window("demo",&source,"other-player",&[110,310],&[]));
        assert!(!ledger.contains_window("demo",&format!("sha256:{}","b".repeat(64)),"player",&[110,310],&[]));
        assert!(!ledger.contains_window("demo",&source,"player",&[290,490],&[]));
        assert!(!ledger.contains_window("demo",&source,"player",&[0,1000],&[]));
        assert!(!already_covered(0,0,0,100));
    }
    #[test]
    fn corrupt_marker_and_unknown_schema_cannot_replace_the_original_history() {
        let root = tempfile::tempdir().unwrap();
        let base = serde_json::json!({"schemaVersion":1,"attempts":[{"kind":"observed","demoId":"demo","sourceFingerprint":format!("sha256:{}","a".repeat(64)),"playerId":"1","momentId":"moment","startTick":100,"endTick":200}],"sourceAliases":{}});
        for field in [
            "schemaVersion",
            "sourceFingerprint",
            "playerId",
            "endTick",
            "unknown",
        ] {
            let mut value = base.clone();
            match field {
                "schemaVersion" => value[field] = serde_json::json!(2),
                "sourceFingerprint" => {
                    value["attempts"][0][field] = serde_json::json!("sha256:not-a-fingerprint")
                }
                "playerId" => value["attempts"][0][field] = serde_json::json!("injected\nplayer"),
                "endTick" => value["attempts"][0][field] = serde_json::json!(99),
                _ => value[field] = serde_json::json!(true),
            }
            let original = serde_json::to_vec(&value).unwrap();
            std::fs::write(root.path().join(FILE), &original).unwrap();
            assert!(migrate(root.path(), &[]).is_err(), "{field}");
            assert_eq!(std::fs::read(root.path().join(FILE)).unwrap(), original);
        }
    }
    #[test]
    fn observed_identity_is_specific_to_source_player_and_moment_but_not_title_or_quality() {
        let mut highlight:Highlight=serde_json::from_value(serde_json::json!({"id":"moment","player":{"steamid":"1","name":"Fixture"},"round":1,"startTick":100,"endTick":200,"anchorTick":150,"score":1,"tags":[],"title":"Original title","kills":[],"breakdown":{}})).unwrap();
        let current = format!("sha256:{}", "a".repeat(64));
        let mut ledger = Ledger::default();
        ledger.add_highlight("demo", &current, &highlight);
        highlight.title = "Different model description".into();
        highlight.score = 100.0;
        assert!(ledger.contains_highlight("renamed-demo", &current, &highlight));
        assert!(!ledger.contains_highlight(
            "demo",
            &format!("sha256:{}", "b".repeat(64)),
            &highlight
        ));
        highlight.player.steamid = "2".into();
        assert!(!ledger.contains_highlight("demo", &current, &highlight));
        highlight.player.steamid = "1".into();
        highlight.id = "another-moment".into();
        highlight.start_tick = 300;
        highlight.end_tick = 400;
        assert!(!ledger.contains_highlight("demo", &current, &highlight));
    }
}
