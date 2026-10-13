//! Thin wrapper over the vendored `parser` crate (LaihoE/demoparser, MIT) that
//! turns a .dem file into the normalized [`DemoData`] contract. One pass reads
//! every game event we need; a second pass samples player team/user_id at the
//! freeze-end tick of every round (events and tick collection cannot share a
//! pass in demoparser).

use crate::model::*;
use ahash::AHashMap;
use anyhow::{anyhow, Context, Result};
use parser::first_pass::parser_settings::FirstPassParser;
use parser::first_pass::parser_settings::{rm_user_friendly_names, ParserInputs};
use parser::parse_demo::{DemoOutput, Parser, ParsingMode};
use parser::second_pass::game_events::GameEvent;
use parser::second_pass::parser_settings::create_huffman_lookup_table;
use parser::second_pass::variants::{VarVec, Variant};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

const PLAYER_EXTRA: &[&str] = &[
    "health",
    "team_num",
    "X",
    "Y",
    "Z",
    "pitch",
    "yaw",
    "is_scoped",
    "team_name",
    "active_weapon_name",
    "fl_recoil_idx",
    "active_weapon_ammo",
    "is_in_reload",
    "is_controlling_bot",
    "has_controlled_bot_this_round",
];
const OTHER_EXTRA: &[&str] = &[
    "total_rounds_played",
    "is_freeze_period",
    "is_warmup_period",
];
const EVENTS: &[&str] = &[
    "player_death",
    "player_hurt",
    "weapon_fire",
    "fire_bullets",
    "player_blind",
    "round_start",
    "round_freeze_end",
    "round_end",
    "round_officially_ended",
    "bomb_planted",
    "bomb_defused",
    "bomb_exploded",
    "cs_win_panel_match",
    "weapon_reload",
    "player_spawn",
    "player_disconnect",
    "player_team",
    "bot_takeover",
];

/// Reusable parser context: the huffman table is expensive to build, so build it once per process.
pub struct DemoParser {
    huffman: Vec<(u8, u8)>,
}

impl Default for DemoParser {
    fn default() -> Self {
        Self::new()
    }
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

impl DemoParser {
    pub fn new() -> Self {
        Self {
            huffman: create_huffman_lookup_table(),
        }
    }

    pub(crate) fn inputs<'a>(
        &'a self,
        player_props: &[String],
        other_props: &[String],
        events: &[String],
        ticks: Vec<i32>,
    ) -> Result<ParserInputs<'a>> {
        self.inputs_ex(player_props, other_props, events, ticks, false)
    }

    fn inputs_ex<'a>(
        &'a self,
        player_props: &[String],
        other_props: &[String],
        events: &[String],
        ticks: Vec<i32>,
        projectiles: bool,
    ) -> Result<ParserInputs<'a>> {
        let real_player =
            rm_user_friendly_names(&player_props.to_vec()).map_err(|e| anyhow!("{e:?}"))?;
        let real_other =
            rm_user_friendly_names(&other_props.to_vec()).map_err(|e| anyhow!("{e:?}"))?;
        let mut real_name_to_og_name = AHashMap::default();
        for (real, friendly) in real_player.iter().zip(player_props) {
            real_name_to_og_name.insert(real.clone(), friendly.clone());
        }
        for (real, friendly) in real_other.iter().zip(other_props) {
            real_name_to_og_name.insert(real.clone(), friendly.clone());
        }
        Ok(ParserInputs {
            real_name_to_og_name,
            wanted_players: vec![],
            wanted_player_props: real_player,
            wanted_other_props: real_other,
            wanted_prop_states: AHashMap::default(),
            wanted_ticks: ticks,
            wanted_events: events.to_vec(),
            parse_ents: true,
            parse_projectiles: projectiles,
            parse_grenades: projectiles,
            only_header: false,
            only_convars: false,
            huffman_lookup_table: &self.huffman,
            order_by_steamid: false,
            list_props: false,
            fallback_bytes: None,
        })
    }

    pub(crate) fn scene_frames(
        &self,
        bytes: &[u8],
        first_tick: i32,
        last_tick: i32,
        step: i32,
    ) -> Result<Vec<crate::analysis::scene::SceneFrame>> {
        self.collect_scene_frames(bytes, first_tick, last_tick, step, false)
    }
    pub fn player_tracking_frames(
        &self,
        bytes: &[u8],
        first_tick: i32,
        last_tick: i32,
        step: i32,
    ) -> Result<Vec<crate::analysis::scene::SceneFrame>> {
        anyhow::ensure!(
            first_tick >= 0 && last_tick >= first_tick && step > 0,
            "invalid tracking interval"
        );
        self.collect_scene_frames(bytes, first_tick, last_tick, step, true)
    }
    fn collect_scene_frames(
        &self,
        bytes: &[u8],
        first_tick: i32,
        last_tick: i32,
        step: i32,
        tracking_only: bool,
    ) -> Result<Vec<crate::analysis::scene::SceneFrame>> {
        let mut frames = Vec::new();
        self.visit_scene_frames(bytes, first_tick, last_tick, step, tracking_only, |frame| {
            frames.push(frame);
            Ok(())
        })?;
        Ok(frames)
    }
    /// Retains only the final packet state for the current tick; the consumer owns its history.
    pub fn visit_scene_frames(
        &self,
        bytes: &[u8],
        first_tick: i32,
        last_tick: i32,
        step: i32,
        tracking_only: bool,
        mut visit: impl FnMut(crate::analysis::scene::SceneFrame) -> Result<()>,
    ) -> Result<()> {
        anyhow::ensure!(
            first_tick >= 0 && last_tick >= first_tick && step > 0,
            "invalid scene interval"
        );
        let inputs = self.inputs(&[], &[], &[], vec![i32::MIN])?;
        let mut first = FirstPassParser::new(&inputs);
        let output = first
            .parse_demo(bytes, true)
            .map_err(|e| anyhow!("scene schema: {e:?}"))?;
        let mut second = parser::second_pass::parser_settings::SecondPassParser::new(
            output,
            parser::first_pass::parser::HEADER_ENDS_AT_BYTE,
            true,
            None,
        )
        .map_err(|e| anyhow!("scene parser: {e:?}"))?;
        second.capture_pose_fields = !tracking_only;
        let mut pending: Option<crate::analysis::scene::SceneFrame> = None;
        let mut failure = None;
        second
            .start_with_observer(bytes, |p| {
                if pending.as_ref().is_some_and(|frame| p.tick > frame.tick) {
                    if let Err(error) = visit(pending.take().unwrap()) {
                        failure = Some(error);
                        return false;
                    }
                }
                if p.tick > last_tick {
                    return false;
                }
                if p.tick >= first_tick && (p.tick - first_tick) % step == 0 {
                    pending = Some(crate::analysis::scene::capture(p, tracking_only));
                }
                true
            })
            .map_err(|e| anyhow!("scene packets: {e:?}"))?;
        if let Some(error) = failure {
            return Err(error);
        }
        if let Some(frame) = pending {
            visit(frame)?;
        }
        Ok(())
    }
    /// Separate, explicitly requested scoring preparation; normal demo parsing is unchanged.
    pub fn write_match_state(&self, bytes: &[u8], output: impl std::io::Write) -> Result<()> {
        let inputs = self.inputs(&[], &[], &["all".into()], vec![i32::MIN])?;
        let mut first = FirstPassParser::new(&inputs);
        let parsed = first
            .parse_demo(bytes, true)
            .map_err(|e| anyhow!("analysis schema: {e:?}"))?;
        let tick_rate = crate::analysis::server_tick_rate(&parsed.server_infos)?;
        let mut writer = crate::analysis::compact::Writer::new(
            output,
            crate::analysis::demo_source(bytes)?,
            tick_rate,
        )?;
        let mut second = parser::second_pass::parser_settings::SecondPassParser::new(
            parsed,
            parser::first_pass::parser::HEADER_ENDS_AT_BYTE,
            true,
            None,
        )
        .map_err(|e| anyhow!("analysis parser: {e:?}"))?;
        second.capture_pose_fields = true;
        second.analysis_changes = Some(Default::default());
        let mut failure = None;
        second
            .start_with_observer(bytes, |p| {
                if let Err(error) = writer.push(p) {
                    failure = Some(error);
                    return false;
                }
                true
            })
            .map_err(|e| anyhow!("analysis packets: {e:?}"))?;
        if let Some(error) = failure {
            return Err(error);
        }
        // An incomplete or unsupported source keeps policy absent, never guessed.
        if let Ok(policies) = crate::analysis::smoke::spread::policies_for_demo(bytes) {
            annotate_smoke_policy(&mut second.game_events, policies);
        }
        writer.events(&second.game_events)?;
        writer.finish()?;
        Ok(())
    }
    pub fn server_info(&self, bytes: &[u8]) -> Result<Vec<csgoproto::CsvcMsgServerInfo>> {
        let inputs = self.inputs(&[], &[], &[], vec![])?;
        let mut first = FirstPassParser::new(&inputs);
        Ok(first
            .parse_demo(bytes, true)
            .map_err(|e| anyhow!("server info: {e:?}"))?
            .server_infos)
    }
    /// Property names actually advertised by this demo's send tables.
    pub fn property_names(&self, bytes: &[u8]) -> Result<Vec<String>> {
        let mut inputs = self.inputs(&[], &[], &[], vec![])?;
        inputs.list_props = true;
        let mut first = FirstPassParser::new(&inputs);
        let out = first
            .parse_demo(bytes, true)
            .map_err(|e| anyhow!("properties: {e:?}"))?;
        let mut names: Vec<_> = out.prop_controller.name_to_id.keys().cloned().collect();
        names.sort();
        Ok(names)
    }
    pub fn header(&self, bytes: &[u8]) -> Result<HashMap<String, String>> {
        let inputs = self.inputs(&[], &[], &[], vec![])?;
        let mut first = FirstPassParser::new(&inputs);
        let header = first
            .parse_header_only(bytes)
            .map_err(|e| anyhow!("header: {e:?}"))?;
        Ok(header.into_iter().collect())
    }

    pub fn events(
        &self,
        bytes: &[u8],
        names: &[String],
        player_extra: &[String],
        other_extra: &[String],
    ) -> Result<DemoOutput> {
        let inputs = self.inputs(player_extra, other_extra, names, vec![])?;
        let mut parser = Parser::new(inputs, ParsingMode::Normal);
        parser
            .parse_demo(bytes)
            .map_err(|e| anyhow!("events: {e:?}"))
    }

    /// One row per (tick, player) with the requested props plus `tick`, `steamid`, `name`.
    pub fn ticks(&self, bytes: &[u8], props: &[String], wanted_ticks: Vec<i32>) -> Result<Rows> {
        let inputs = self.inputs(props, &[], &[], wanted_ticks)?;
        let mut parser = Parser::new(inputs, ParsingMode::Normal);
        let out = parser
            .parse_demo(bytes)
            .map_err(|e| anyhow!("ticks: {e:?}"))?;
        Ok(Rows::from_output(out))
    }

    /// One row per (tick, projectile or dropped equipment) with `grenade_type`, `grenade_entity_id`,
    /// `x`, `y`, `z`, `tick`, `steamid` (thrower), `name`.
    /// Dropped items use `dropped:<weapon name>` as their grenade_type.
    pub fn projectiles(&self, bytes: &[u8], wanted_ticks: Vec<i32>) -> Result<Rows> {
        let inputs = self.inputs_ex(
            &[
                "m_firePositions".into(),
                "m_fireCount".into(),
                "m_nInfernoType".into(),
                "m_nVoxelFrameDataSize".into(),
            ],
            &[],
            &[],
            wanted_ticks,
            true,
        )?;
        let mut parser = Parser::new(inputs, ParsingMode::Normal);
        let out = parser
            .parse_demo(bytes)
            .map_err(|e| anyhow!("projectiles: {e:?}"))?;
        Ok(Rows::from_output(out))
    }

    /// Full [`DemoData`] for a demo file.
    pub fn load_demo(&self, path: &Path) -> Result<DemoData> {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        self.load_demo_bytes(path, &bytes)
    }

    pub fn load_demo_bytes(&self, path: &Path, bytes: &[u8]) -> Result<DemoData> {
        let header = self.header(bytes)?;
        let out = self.events(
            bytes,
            &strings(EVENTS),
            &strings(PLAYER_EXTRA),
            &strings(OTHER_EXTRA),
        )?;

        let tick_rate = crate::analysis::server_tick_rate(&out.server_infos)?;
        let mut players: Vec<PlayerInfo> = if out.player_md.is_empty() {
            &out.roster
        } else {
            &out.player_md
        }
        .iter()
        .filter_map(|p| {
            Some(PlayerInfo {
                name: p.name.clone()?,
                steamid: p.steamid?.to_string(),
                team_number: p.team_number.unwrap_or(0),
                user_id: None,
            })
        })
        .collect();

        let mut groups: HashMap<&str, Vec<&GameEvent>> = HashMap::new();
        for ev in &out.game_events {
            groups.entry(ev.name.as_str()).or_default().push(ev);
        }
        let kills = kills_from_events(
            groups
                .get("player_death")
                .map(|v| v.as_slice())
                .unwrap_or(&[]),
        );
        let demo_end_tick = crate::demo_readiness::end_tick_bytes(bytes)?;
        let (mut rounds, defusers) = rounds_from_events(&groups, demo_end_tick);

        let mut cash = BTreeMap::new();
        // Roster + user ids at every freeze end.
        let wanted: Vec<i32> = rounds.iter().map(|r| r.freeze_end_tick + 1).collect();
        if !wanted.is_empty() {
            let rows = self.ticks(bytes, &strings(&["team_num", "user_id", "balance"]), wanted)?;
            let mut by_tick: HashMap<i32, Vec<Row>> = HashMap::new();
            let mut user_ids: HashMap<String, i32> = HashMap::new();
            for row in rows.iter() {
                let Some(tick) = row.tick() else { continue };
                by_tick.entry(tick).or_default().push(row);
                if let (Some(sid), Some(uid)) = (row.steamid(), row.num("user_id")) {
                    user_ids.insert(sid, uid as i32);
                }
            }
            for r in &mut rounds {
                let mut roster = BTreeMap::new();
                for row in by_tick
                    .get(&(r.freeze_end_tick + 1))
                    .map(|v| v.as_slice())
                    .unwrap_or(&[])
                {
                    let team = match row.num("team_num").map(|n| n as i32) {
                        Some(2) => Team::T,
                        Some(3) => Team::Ct,
                        _ => continue,
                    };
                    if let Some(sid) = row.steamid() {
                        if let Some(value) = row.num("balance") {
                            cash.insert((r.round, sid.clone()), value.max(0.0) as u32);
                        }
                        roster.insert(sid, team);
                    }
                }
                r.roster = roster;
            }
            for p in &mut players {
                p.user_id = user_ids.get(&p.steamid).copied();
            }
        }
        let mut clutch_ineligible_rounds = vec![];
        for r in &mut rounds {
            r.bomb_defuser = r.bomb_defused_tick.and_then(|t| defusers.get(&t).cloned());
            // A fixed freeze-end roster cannot prove a clutch through a live
            // respawn, disconnect, team change or controlled-bot transition.
            let changed = [
                "player_spawn",
                "player_disconnect",
                "player_team",
                "bot_takeover",
            ]
            .iter()
            .any(|name| {
                groups.get(*name).into_iter().flatten().any(|ev| {
                    Fields(ev).tick() > r.freeze_end_tick && Fields(ev).tick() < r.end_tick
                })
            });
            if changed {
                clutch_ineligible_rounds.push(r.round);
            }
        }

        let demo_digest = format!("{:x}", Sha256::digest(bytes));
        let recorded_deaths = death_ledger(&groups, &rounds, &demo_digest);
        let event_ledger = discovery_ledger(&out.game_events, &rounds, &demo_digest);

        let (damage, damage_ticks) = damage_from_events(
            groups
                .get("player_hurt")
                .map(|v| v.as_slice())
                .unwrap_or(&[]),
            &rounds,
        );

        let mut recoil = crate::aim::recoil(&out.game_events, &rounds, tick_rate);
        crate::aim::tracking::attach(self, bytes, &mut recoil, &out.game_events, tick_rate)?;
        let aim = crate::aim::compute(&out.game_events, &rounds, tick_rate);
        let activity = activity_from_events(&out.game_events, &rounds);
        let round_metrics = rounds
            .iter()
            .map(|r| {
                let window = std::slice::from_ref(r);
                let (damage, _) = damage_from_events(
                    groups
                        .get("player_hurt")
                        .map(|v| v.as_slice())
                        .unwrap_or(&[]),
                    window,
                );
                let activity = activity_from_events(&out.game_events, window);
                let metrics = players
                    .iter()
                    .map(|p| {
                        (
                            p.steamid.clone(),
                            RoundMetrics {
                                cash: cash.get(&(r.round, p.steamid.clone())).copied(),
                                damage: damage.get(&p.steamid).map_or(0, |d| d.total),
                                flashed: activity.get(&p.steamid).map_or(0, |a| a.enemies_flashed),
                                ..Default::default()
                            },
                        )
                    })
                    .collect();
                (r.round, metrics)
            })
            .collect();
        Ok(DemoData {
            demo_digest,
            demo_end_tick: demo_end_tick.unwrap_or_else(|| kills.last().map_or(0, |k| k.tick)),
            event_ledger,
            event_ledger_version: 1,
            recorded_deaths,
            clutch_ineligible_rounds,
            round_metrics,
            info: DemoInfo {
                path: path.to_string_lossy().to_string(),
                map_name: header.get("map_name").cloned().unwrap_or_default(),
                server_name: header.get("server_name").cloned().unwrap_or_default(),
                tick_rate,
                players,
            },
            kills,
            rounds,
            damage,
            damage_ticks,
            activity,
            aim,
            recoil,
            recoil_reference: Default::default(), // Legacy cache field; the chart uses a bundled calibration.
        })
    }
}

/// Normalizes discovery facts during the already-required event pass, so review
/// planning never reparses a demo or requires CS2 to play through the match.
fn discovery_ledger(events: &[GameEvent], rounds: &[RoundInfo], digest: &str) -> Vec<DiscoveryEvent> {
    let mut ledger = vec![];
    for event in events {
        if !matches!(event.name.as_str(), "weapon_fire" | "player_hurt" | "player_blind"
            | "player_death" | "bomb_planted" | "bomb_defused" | "bomb_exploded" | "weapon_reload") {
            continue;
        }
        let f = Fields(event);
        if f.bool("is_freeze_period") || f.bool("is_warmup_period") { continue; }
        let Some(round) = rounds.iter().find(|r| f.tick() >= r.freeze_end_tick && f.tick() <= r.officially_ended_tick) else { continue; };
        let actor_field = if matches!(event.name.as_str(), "player_hurt" | "player_blind" | "player_death") { "attacker_steamid" } else { "user_steamid" };
        let player_id = f.str(actor_field);
        let target = matches!(event.name.as_str(), "player_hurt" | "player_blind" | "player_death").then(|| f.str("user_steamid")).filter(|id| !id.is_empty());
        let relation = target.as_ref().map(|target| {
            if target == &player_id { "self" }
            else {
                let sides = (round.roster.get(&player_id), round.roster.get(target));
                match sides { (Some(a), Some(b)) if a == b => "teammate", (Some(_), Some(_)) => "enemy", _ => "unknown" }
            }
        }).unwrap_or("none").to_string();
        let value = match event.name.as_str() {
            "player_hurt" => f.opt_num("dmg_health").filter(|n| n.is_finite() && *n > 0.0),
            "player_blind" => f.opt_num("blind_duration").filter(|n| n.is_finite() && *n > 0.0),
            _ => None,
        };
        // World deaths remain represented by the target; unidentified fire
        // cannot safely be associated with the selected person's POV.
        if player_id.is_empty() && target.is_none() && !event.name.starts_with("bomb_") { continue; }
        let raw_weapon = f.str("weapon");
        let weapon = raw_weapon.strip_prefix("weapon_").unwrap_or(&raw_weapon).to_string();
        let id = format!("event-{:x}", Sha256::digest(format!("{digest}:{}:{}:{}:{}:{}", f.tick(), event.name, player_id, target.as_deref().unwrap_or(""), ledger.len())));
        ledger.push(DiscoveryEvent { id, tick: f.tick(), round: round.round, kind: event.name.clone(), player_id, target_id: target, weapon, value, relation });
    }
    ledger.sort_by_key(|event| event.tick);
    ledger
}

#[cfg(test)]
mod discovery_ledger_tests {
    use super::*;
    use parser::second_pass::game_events::EventField;

    #[test]
    fn ledger_normalizes_weapon_and_recorded_effect_without_inventing_visibility_or_including_warmup() {
        let round:RoundInfo=serde_json::from_value(serde_json::json!({"round":1,"startTick":0,"freezeEndTick":100,"endTick":1000,"officiallyEndedTick":1100,"reason":"","roster":{"p":"CT","e":"TERRORIST","friend":"CT"}})).unwrap();
        let event=|name:&str,tick:i32,actor:&str,target:&str,warmup:bool|GameEvent{name:name.into(),tick,fields:vec![
            EventField{name:"user_steamid".into(),data:Some(Variant::String(if name=="weapon_fire"{actor}else{target}.into()))},
            EventField{name:"attacker_steamid".into(),data:Some(Variant::String(actor.into()))},
            EventField{name:"weapon".into(),data:Some(Variant::String("weapon_ak47".into()))},
            EventField{name:"dmg_health".into(),data:Some(Variant::I32(25))},
            EventField{name:"is_warmup_period".into(),data:Some(Variant::Bool(warmup))}]};
        let events=[event("weapon_fire",150,"p","",false),event("player_hurt",150,"p","e",false),event("player_hurt",151,"p","friend",false),
            event("weapon_fire",140,"p","",true),event("weapon_fire",50,"p","",false),event("weapon_fire",1200,"p","",false)];
        let ledger=discovery_ledger(&events,&[round],"source");
        assert_eq!(ledger.len(),3);assert_eq!(ledger[0].weapon,"ak47");assert_eq!(ledger[0].value,None);
        assert_eq!(ledger[1].relation,"enemy");assert_eq!(ledger[1].value,Some(25.0));assert_eq!(ledger[2].relation,"teammate");
        assert_ne!(ledger[0].id,ledger[1].id);assert!(ledger.iter().all(|e|e.round==1));
    }

    #[test]
    #[ignore = "Set CLIPFORGE_TEST_DEMO to a workspace demo; CPU-only real match acceptance."]
    fn real_match_discovery_ledger_and_targeted_plan() {
        let source = std::env::var("CLIPFORGE_TEST_DEMO").expect("CLIPFORGE_TEST_DEMO");
        let root = Path::new("J:/CS2ClipStudio").canonicalize().unwrap();
        let path = Path::new(&source).canonicalize().unwrap();assert!(path.starts_with(root));
        let started=std::time::Instant::now();
        let parsed=crate::stats::build_parsed_demo(DemoParser::new().load_demo(&path).unwrap());
        assert_eq!(parsed.event_ledger_version,1);assert!(!parsed.event_ledger.is_empty());
        assert!(parsed.event_ledger.windows(2).all(|pair|pair[0].tick<=pair[1].tick));
        let mut total_windows=0;let mut preview=0.0;let mut full=0.0;
        for player in &parsed.info.players {
            let plan=crate::detector::review_plan(&parsed,&player.steamid,"targeted").unwrap();
            assert!(plan.candidates.iter().all(|c|c.player_id==player.steamid&&c.source_digest==parsed.demo_digest));
            assert!(plan.windows.iter().all(|w|w.start_tick>=0&&w.end_tick>w.start_tick&&w.end_tick<=parsed.demo_end_tick));
            total_windows+=plan.windows.len();preview+=plan.preview_seconds;full+=plan.full_match_seconds;
        }
        println!("real CPU discovery: ledger={} candidates={} players={} batches={} aggregate-preview-seconds={:.1} aggregate-full-seconds={:.1} elapsed={:.1?}",
            parsed.event_ledger.len(),parsed.discovery_candidates.len(),parsed.info.players.len(),total_windows,preview,full,started.elapsed());
        assert!(!parsed.discovery_candidates.is_empty());assert!(preview<full);
    }
}

/// Column-major result of a tick / projectile pass, taken straight out of the
/// parser's data frame (no per-row allocation). Rows are visited in tick order.
pub struct Rows {
    cols: Vec<VarVec>,
    index: HashMap<String, usize>,
    /// row indices sorted by tick
    order: Vec<usize>,
}

/// One row of [`Rows`]; accessors tolerate the parser's variant choices.
#[derive(Clone, Copy)]
pub struct Row<'a> {
    rows: &'a Rows,
    i: usize,
}

impl Rows {
    fn from_output(mut out: DemoOutput) -> Self {
        let mut cols = vec![];
        let mut index = HashMap::new();
        let mut len = 0;
        for info in &out.prop_controller.prop_infos {
            let Some(col) = out.df.remove(&info.id) else {
                continue;
            };
            len = len.max(col.data.as_ref().map(col_len).unwrap_or(col.num_nones));
            if let Some(data) = col.data {
                index.insert(info.prop_friendly_name.clone(), cols.len());
                cols.push(data);
            }
        }
        let mut rows = Self {
            cols,
            index,
            order: (0..len).collect(),
        };
        let ticks: Vec<i32> = rows.iter().map(|r| r.tick().unwrap_or(0)).collect();
        rows.order.sort_by_key(|i| ticks[*i]);
        rows
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = Row<'_>> {
        (0..self.order.len()).map(move |i| Row { rows: self, i })
    }
}

fn col_len(v: &VarVec) -> usize {
    match v {
        VarVec::Bool(v) => v.len(),
        VarVec::U32(v) => v.len(),
        VarVec::I32(v) => v.len(),
        VarVec::F32(v) => v.len(),
        VarVec::U64(v) => v.len(),
        VarVec::String(v) => v.len(),
        VarVec::StringVec(v) => v.len(),
        VarVec::XYZVec(v) => v.len(),
        _ => 0,
    }
}

impl<'a> Row<'a> {
    fn col(&self, key: &str) -> Option<(&'a VarVec, usize)> {
        let col = &self.rows.cols[*self.rows.index.get(key)?];
        Some((col, self.rows.order[self.i]))
    }

    pub fn num(&self, key: &str) -> Option<f64> {
        let (col, i) = self.col(key)?;
        match col {
            VarVec::F32(v) => v.get(i).copied().flatten().map(f64::from),
            VarVec::I32(v) => v.get(i).copied().flatten().map(f64::from),
            VarVec::U32(v) => v.get(i).copied().flatten().map(f64::from),
            VarVec::U64(v) => v.get(i).copied().flatten().map(|v| v as f64),
            VarVec::Bool(v) => v.get(i).copied().flatten().map(|b| b as i32 as f64),
            _ => None,
        }
    }

    /// Missing vectors stay missing; do not replace absent positions with zero.
    pub fn vec3(&self, key: &str) -> Option<[f64; 3]> {
        match self.col(key)? {
            (VarVec::XYZVec(v), i) => v.get(i).copied().flatten().map(|p| p.map(f64::from)),
            _ => None,
        }
    }
    pub fn flag(&self, key: &str) -> bool {
        self.num(key).map(|v| v != 0.0).unwrap_or(false)
    }

    pub fn str(&self, key: &str) -> Option<&'a str> {
        match self.col(key)? {
            (VarVec::String(v), i) => v.get(i)?.as_deref(),
            _ => None,
        }
    }

    pub fn strs(&self, key: &str) -> Option<&'a [String]> {
        match self.col(key)? {
            (VarVec::StringVec(v), i) => v.get(i).map(Vec::as_slice),
            _ => None,
        }
    }

    pub fn tick(&self) -> Option<i32> {
        match self.col("tick")? {
            (VarVec::I32(v), i) => v.get(i).copied().flatten(),
            _ => None,
        }
    }

    /// Steamid as the parser stored it (u64 or string); bots carry `0`.
    pub fn steamid(&self) -> Option<String> {
        match self.col("steamid")? {
            (VarVec::U64(v), i) => v.get(i).copied().flatten().map(|v| v.to_string()),
            (VarVec::String(v), i) => v.get(i)?.clone(),
            _ => None,
        }
    }
}

/// Game-event field accessors tolerant to the parser's variant choices; the
/// non-`opt_` variants substitute a zero / empty default.
pub(crate) struct Fields<'a>(pub &'a GameEvent);

impl<'a> Fields<'a> {
    fn get(&self, name: &str) -> Option<&'a Variant> {
        self.0
            .fields
            .iter()
            .find(|f| f.name == name)
            .and_then(|f| f.data.as_ref())
    }
    pub fn opt_str(&self, name: &str) -> Option<String> {
        match self.get(name)? {
            Variant::String(s) => Some(s.clone()),
            Variant::U64(v) => Some(v.to_string()),
            Variant::I32(v) => Some(v.to_string()),
            Variant::U32(v) => Some(v.to_string()),
            _ => None,
        }
    }
    pub fn str(&self, name: &str) -> String {
        self.opt_str(name).unwrap_or_default()
    }
    pub fn opt_num(&self, name: &str) -> Option<f64> {
        match self.get(name)? {
            Variant::F32(v) => Some(*v as f64),
            Variant::I32(v) => Some(*v as f64),
            Variant::U32(v) => Some(*v as f64),
            Variant::U64(v) => Some(*v as f64),
            Variant::Bool(b) => Some(*b as i32 as f64),
            _ => None,
        }
    }
    pub fn num(&self, name: &str) -> f64 {
        self.opt_num(name).unwrap_or(0.0)
    }
    pub fn int(&self, name: &str) -> i32 {
        self.num(name) as i32
    }
    pub fn opt_bool(&self, name: &str) -> Option<bool> {
        match self.get(name)? {
            Variant::Bool(b) => Some(*b),
            Variant::I32(v) => Some(*v != 0),
            Variant::U32(v) => Some(*v != 0),
            Variant::String(s) => Some(s == "true" || s == "1"),
            _ => None,
        }
    }
    pub fn bool(&self, name: &str) -> bool {
        self.opt_bool(name).unwrap_or(false)
    }
    /// Event tick; falls back to the message tick when the event carries none.
    pub fn tick(&self) -> i32 {
        match self.int("tick") {
            0 => self.0.tick,
            t => t,
        }
    }
}

fn kills_from_events(events: &[&GameEvent]) -> Vec<KillEvent> {
    let mut kills: Vec<KillEvent> = events
        .iter()
        .map(|ev| {
            let f = Fields(ev);
            let attacker_id = f.str("attacker_steamid");
            let assister_id = f.str("assister_steamid");
            KillEvent {
                tick: f.tick(),
                round: f.int("total_rounds_played"),
                attacker: (!attacker_id.is_empty()).then(|| Attacker {
                    steamid: attacker_id.clone(),
                    name: f.str("attacker_name"),
                    team: Team::from_name(&f.str("attacker_team_name")),
                    health: f.int("attacker_health"),
                    weapon_name: f.str("attacker_active_weapon_name"),
                }),
                victim: Victim {
                    steamid: f.str("user_steamid"),
                    name: f.str("user_name"),
                    team: Team::from_name(&f.str("user_team_name")),
                    weapon_name: f.str("user_active_weapon_name"),
                },
                assister: (!assister_id.is_empty()).then(|| Assister {
                    steamid: assister_id.clone(),
                    name: f.str("assister_name"),
                    team: Team::from_name(&f.str("assister_team_name")),
                }),
                weapon: f.str("weapon"),
                headshot: f.bool("headshot"),
                noscope: f.bool("noscope"),
                penetrated: f.int("penetrated"),
                thru_smoke: f.bool("thrusmoke"),
                attacker_blind: f.bool("attackerblind"),
                attacker_in_air: f.bool("attackerinair"),
                assisted_flash: f.bool("assistedflash"),
                distance: f.num("distance"),
                hitgroup: f.str("hitgroup"),
                is_freeze_period: f.bool("is_freeze_period"),
            }
        })
        .collect();
    kills.sort_by_key(|k| k.tick);
    kills
}

fn death_ledger(
    groups: &HashMap<&str, Vec<&GameEvent>>,
    rounds: &[RoundInfo],
    digest: &str,
) -> Vec<RecordedDeath> {
    let mut events = groups.get("player_death").cloned().unwrap_or_default();
    events.sort_by_key(|ev| Fields(ev).tick());
    let match_end = groups
        .get("cs_win_panel_match")
        .and_then(|events| events.iter().map(|ev| Fields(ev).tick()).min());
    let hurt = groups.get("player_hurt");
    let mut ordinals: HashMap<(i32, String), usize> = HashMap::new();
    events
        .into_iter()
        .map(|ev| {
            let f = Fields(ev);
            let kill = kills_from_events(&[ev]).remove(0);
            let sid = &kill.victim.steamid;
            let tick = kill.tick;
            let round = rounds
                .iter()
                .rev()
                .find(|r| tick >= r.start_tick && tick <= r.officially_ended_tick);
            let phase = if f.opt_bool("is_warmup_period") == Some(true) {
                "warmup"
            } else if match_end.is_some_and(|end| tick > end) {
                "post-match"
            } else if let Some(r) = round {
                if tick < r.freeze_end_tick {
                    "freeze"
                } else if tick <= r.end_tick {
                    "competitive"
                } else {
                    "post-round"
                }
            } else {
                "unknown"
            };
            let mut tags = vec!["death".to_string(), phase.to_string()];
            if kill.weapon == "world" {
                tags.push("world-damage".into());
            }
            // A generic world death does not identify fall damage.
            if matches!(kill.weapon.as_str(), "fall" | "fall_damage" | "falldamage") {
                tags.push("fall-damage".into());
            }
            if let Some(attacker) = &kill.attacker {
                if attacker.steamid == *sid {
                    tags.push("self-death".into());
                } else if attacker.team == kill.victim.team {
                    tags.push("team-death".into());
                }
            }
            if f.opt_bool("user_is_in_reload") == Some(true) {
                tags.push("died-while-reloading".into());
            }
            if f.opt_num("user_active_weapon_ammo") == Some(0.0)
                && is_recorded_firearm(&kill.victim.weapon_name)
            {
                tags.push("empty-magazine-at-death".into());
            }
            if phase == "competitive" && f.opt_bool("is_warmup_period") == Some(false) {
                if let (Some(r), Some(hurt)) = (round, hurt) {
                    let dealt_damage = hurt.iter().any(|ev| {
                        let h = Fields(ev);
                        let at = h.int("attacker_team_num");
                        let vt = h.int("user_team_num");
                        h.tick() >= r.freeze_end_tick
                            && h.tick() <= tick
                            && h.str("attacker_steamid") == *sid
                            && matches!(at, 2 | 3)
                            && matches!(vt, 2 | 3)
                            && at != vt
                            && h.int("dmg_health") > 0
                    });
                    if !dealt_damage {
                        tags.push("no-recorded-outgoing-damage-this-round".into());
                    }
                }
            }
            let ordinal = ordinals.entry((tick, sid.clone())).or_default();
            let identity = format!("{digest}:death:{tick}:{sid}:{ordinal}");
            *ordinal += 1;
            RecordedDeath {
                id: format!("death-{:x}", Sha256::digest(identity.as_bytes())),
                kill,
                phase: phase.into(),
                round_number: round.map(|r| r.round),
                tags,
            }
        })
        .collect()
}

fn is_recorded_firearm(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "ak-47"
            | "ak47"
            | "m4a1"
            | "m4a4"
            | "m4a1-s"
            | "m4a1_silencer"
            | "awp"
            | "ssg 08"
            | "ssg08"
            | "aug"
            | "sg 553"
            | "sg556"
            | "scar-20"
            | "scar20"
            | "g3sg1"
            | "galil ar"
            | "galilar"
            | "famas"
            | "usp-s"
            | "usp_silencer"
            | "glock-18"
            | "glock"
            | "p2000"
            | "hkp2000"
            | "p250"
            | "five-seven"
            | "fiveseven"
            | "tec-9"
            | "tec9"
            | "cz75-auto"
            | "cz75a"
            | "desert eagle"
            | "deagle"
            | "dual berettas"
            | "elite"
            | "r8 revolver"
            | "revolver"
            | "mac-10"
            | "mac10"
            | "mp9"
            | "mp7"
            | "mp5-sd"
            | "mp5sd"
            | "ump-45"
            | "ump45"
            | "p90"
            | "pp-bizon"
            | "bizon"
            | "nova"
            | "xm1014"
            | "mag-7"
            | "mag7"
            | "sawed-off"
            | "sawedoff"
            | "m249"
            | "negev"
    )
}

#[cfg(test)]
mod death_tests {
    use super::*;
    use parser::second_pass::game_events::EventField;

    fn event(name: &str, tick: i32, fields: Vec<(&str, Variant)>) -> GameEvent {
        GameEvent {
            name: name.into(),
            tick,
            fields: fields
                .into_iter()
                .map(|(name, value)| EventField {
                    name: name.into(),
                    data: Some(value),
                })
                .collect(),
        }
    }
    fn death(tick: i32, weapon: &str, warmup: bool) -> GameEvent {
        event(
            "player_death",
            tick,
            vec![
                ("user_steamid", Variant::String("ct1".into())),
                ("user_name", Variant::String("Victim".into())),
                ("user_team_name", Variant::String("CT".into())),
                ("user_active_weapon_name", Variant::String("USP-S".into())),
                ("attacker_steamid", Variant::String("t1".into())),
                ("attacker_team_name", Variant::String("TERRORIST".into())),
                ("weapon", Variant::String(weapon.into())),
                ("is_warmup_period", Variant::Bool(warmup)),
            ],
        )
    }
    fn round() -> RoundInfo {
        RoundInfo {
            round: 1,
            start_tick: 10,
            freeze_end_tick: 20,
            end_tick: 100,
            officially_ended_tick: 130,
            winner: Some(Team::Ct),
            reason: String::new(),
            roster: [("ct1".into(), Team::Ct), ("t1".into(), Team::T)].into(),
            bomb_planted_tick: None,
            bomb_defused_tick: None,
            bomb_defuser: None,
            bomb_exploded_tick: None,
        }
    }
    #[test]
    fn qualifier_evidence_and_phase_boundaries_are_conservative() {
        let mut reload = death(60, "ak47", false);
        reload.fields.extend([
            EventField {
                name: "user_active_weapon_ammo".into(),
                data: Some(Variant::U32(0)),
            },
            EventField {
                name: "user_is_in_reload".into(),
                data: Some(Variant::Bool(true)),
            },
        ]);
        let events = vec![
            event(
                "player_hurt",
                40,
                vec![
                    ("attacker_steamid", Variant::String("ct1".into())),
                    ("attacker_team_num", Variant::U32(3)),
                    ("user_team_num", Variant::U32(2)),
                    ("dmg_health", Variant::I32(10)),
                ],
            ),
            death(5, "world", true),
            reload,
            event("cs_win_panel_match", 125, vec![]),
            death(140, "world", false),
            death(140, "world", false),
        ];
        let mut groups: HashMap<&str, Vec<&GameEvent>> = HashMap::new();
        for ev in &events {
            groups.entry(&ev.name).or_default().push(ev);
        }
        let ledger = death_ledger(&groups, &[round()], "digest");
        assert_eq!(ledger.len(), 4);
        assert_eq!(ledger[0].phase, "warmup");
        assert_eq!(ledger[1].phase, "competitive");
        assert!(ledger[1].tags.contains(&"died-while-reloading".into()));
        assert!(ledger[1].tags.contains(&"empty-magazine-at-death".into()));
        assert!(!ledger[1]
            .tags
            .contains(&"no-recorded-outgoing-damage-this-round".into()));
        assert_eq!(ledger[2].phase, "post-match");
        assert_ne!(ledger[2].id, ledger[3].id);
        assert!(ledger
            .iter()
            .all(|d| !d.tags.contains(&"fall-damage".into())));
        // Missing ammo is unknown, rather than an empty magazine.
        assert!(!ledger[2].tags.contains(&"empty-magazine-at-death".into()));
        assert!(!is_recorded_firearm("knife"));
    }
}

const UTILITY_WEAPONS: &[&str] = &[
    "hegrenade",
    "inferno",
    "molotov",
    "incgrenade",
    "decoy",
    "flashbang",
    "smokegrenade",
];

/// Count actual enemy HP lost within the same round windows used for kills.
fn damage_from_events(
    events: &[&GameEvent],
    rounds: &[RoundInfo],
) -> (BTreeMap<String, DamageTotals>, BTreeMap<SteamId, Vec<i32>>) {
    let mut out: BTreeMap<String, DamageTotals> = BTreeMap::new();
    let mut ticks: BTreeMap<SteamId, Vec<i32>> = BTreeMap::new();
    let mut health: HashMap<(usize, String), u32> = HashMap::new();
    let mut events = events.to_vec();
    events.sort_by_key(|ev| Fields(ev).tick());
    for ev in events {
        let f = Fields(ev);
        let Some((index, round)) = rounds
            .iter()
            .enumerate()
            .find(|(_, r)| f.tick() >= r.start_tick && f.tick() <= r.officially_ended_tick)
        else {
            continue;
        };
        let victim = f.str("user_steamid");
        if victim.is_empty() {
            continue;
        }
        let raw = f.int("dmg_health").max(0) as u32;
        let remaining = health
            .entry((index, victim.clone()))
            .or_insert_with(|| f.opt_num("user_health").unwrap_or(100.0).max(0.0) as u32);
        let after = f
            .opt_num("health")
            .map(|n| n.max(0.0) as u32)
            .unwrap_or_else(|| remaining.saturating_sub(raw));
        // dmg_health includes overkill. Entity health can lag multiple hits in one tick,
        // so carry forward the previous hurt event's remaining health instead.
        let damage = raw.min(remaining.saturating_sub(after));
        *remaining = after;

        // Even excluded damage reduces the HP available to subsequent enemy hits.
        let attacker = f.str("attacker_steamid");
        if attacker.is_empty() || attacker == victim || f.bool("is_freeze_period") || damage == 0 {
            continue;
        }
        let team = |field, player: &str| match f.int(field) {
            2 => Some(Team::T),
            3 => Some(Team::Ct),
            _ => round.roster.get(player).copied(),
        };
        let (Some(at), Some(vt)) = (
            team("attacker_team_num", &attacker),
            team("user_team_num", &victim),
        ) else {
            continue;
        };
        let entry = out.entry(attacker.clone()).or_default();
        if at == vt {
            entry.friendly += damage;
            continue;
        }
        ticks.entry(attacker).or_default().push(f.tick());
        ticks.entry(victim).or_default().push(f.tick());
        entry.total += damage;
        match f.str("weapon").as_str() {
            "hegrenade" => entry.he += damage,
            "inferno" | "molotov" | "incgrenade" => entry.fire += damage,
            _ => {}
        }
        if UTILITY_WEAPONS.contains(&f.str("weapon").as_str()) {
            entry.utility += damage;
        }
    }
    for player_ticks in ticks.values_mut() {
        player_ticks.dedup();
    }
    (out, ticks)
}

fn ticks_of(groups: &HashMap<&str, Vec<&GameEvent>>, name: &str) -> Vec<i32> {
    let mut v: Vec<i32> = groups
        .get(name)
        .map(|g| g.iter().map(|e| Fields(e).tick()).collect())
        .unwrap_or_default();
    v.sort_unstable();
    v
}

fn rounds_from_events(
    groups: &HashMap<&str, Vec<&GameEvent>>,
    demo_end_tick: Option<i32>,
) -> (Vec<RoundInfo>, HashMap<i32, String>) {
    let starts: Vec<i32> = ticks_of(groups, "round_start")
        .into_iter()
        .filter(|tick| {
            !groups.get("round_start").into_iter().flatten().any(|ev| {
                Fields(ev).tick() == *tick && Fields(ev).opt_bool("is_warmup_period") == Some(true)
            })
        })
        .collect();
    let freeze_ends = ticks_of(groups, "round_freeze_end");
    let officially = ticks_of(groups, "round_officially_ended");
    let planted = ticks_of(groups, "bomb_planted");
    let exploded = ticks_of(groups, "bomb_exploded");
    let mut defusers: HashMap<i32, String> = HashMap::new();
    let mut defused: Vec<i32> = vec![];
    for ev in groups
        .get("bomb_defused")
        .map(|v| v.as_slice())
        .unwrap_or(&[])
    {
        let f = Fields(ev);
        defused.push(f.tick());
        defusers.insert(f.tick(), f.str("user_steamid"));
    }
    defused.sort_unstable();

    let mut ends: Vec<(i32, i32, String, String)> = groups
        .get("round_end")
        .map(|v| {
            v.iter()
                .filter(|e| Fields(e).opt_bool("is_warmup_period") != Some(true))
                .map(|e| {
                    (
                        Fields(e).tick(),
                        Fields(e).int("round"),
                        Fields(e).str("winner"),
                        Fields(e).str("reason"),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    ends.sort_by_key(|e| e.0);

    let last_before =
        |arr: &[i32], tick: i32| arr.iter().copied().take_while(|t| *t <= tick).last();
    let first_after = |arr: &[i32], tick: i32| arr.iter().copied().find(|t| *t >= tick);
    let between =
        |arr: &[i32], from: i32, to: i32| arr.iter().copied().find(|t| *t >= from && *t <= to);

    let rounds = ends
        .into_iter()
        .map(|(end_tick, round, winner, reason)| {
            let start_tick = last_before(&starts, end_tick).unwrap_or(0);
            let freeze_end_tick = between(&freeze_ends, start_tick, end_tick).unwrap_or(start_tick);
            // The final round can omit round_officially_ended while the demo keeps playing.
            let boundary = starts
                .iter()
                .copied()
                .find(|&t| t > end_tick)
                .into_iter()
                .chain(demo_end_tick)
                .min();
            let officially_ended_tick = first_after(&officially, end_tick)
                .or(boundary)
                .unwrap_or(end_tick)
                .min(boundary.unwrap_or(i32::MAX))
                .max(end_tick);
            RoundInfo {
                round,
                start_tick,
                freeze_end_tick,
                end_tick,
                officially_ended_tick,
                winner: match winner.as_str() {
                    "CT" => Some(Team::Ct),
                    "T" | "TERRORIST" => Some(Team::T),
                    _ => None,
                },
                reason,
                roster: BTreeMap::new(),
                bomb_planted_tick: between(&planted, start_tick, end_tick),
                bomb_defused_tick: between(&defused, start_tick, end_tick),
                bomb_defuser: None,
                bomb_exploded_tick: between(&exploded, start_tick, end_tick),
            }
        })
        .collect();
    (rounds, defusers)
}

#[cfg(test)]
mod round_tests {
    use super::*;
    use parser::second_pass::game_events::EventField;

    fn event(name: &str, tick: i32) -> GameEvent {
        GameEvent {
            name: name.into(),
            tick,
            fields: vec![EventField {
                name: "round".into(),
                data: Some(Variant::I32(1)),
            }],
        }
    }

    #[test]
    fn final_kill_keeps_tail_without_official_round_end() {
        let events = [event("round_start", 100), event("round_end", 1000)];
        let mut groups: HashMap<&str, Vec<&GameEvent>> = HashMap::new();
        for ev in &events {
            groups.entry(&ev.name).or_default().push(ev);
        }
        let (rounds, _) = rounds_from_events(&groups, Some(2280));
        let demo: DemoData = serde_json::from_value(serde_json::json!({
            "info": {"path": "test.dem", "mapName": "de_test", "serverName": "",
                "tickRate": 64.0, "players": []},
            "rounds": rounds,
            "kills": [{"tick": 1000, "round": 0,
                "attacker": {"steamid": "ct1", "name": "Player", "team": "CT",
                    "health": 100, "weaponName": "AK-47"},
                "victim": {"steamid": "t1", "name": "Opponent", "team": "TERRORIST", "weaponName": "AK-47"},
                "weapon": "ak47", "headshot": true, "noscope": false, "penetrated": 0,
                "thruSmoke": false, "attackerBlind": false, "attackerInAir": false,
                "assistedFlash": false, "distance": 10.0, "hitgroup": "head", "isFreezePeriod": false}]
        })).unwrap();
        let highlights = crate::detector::detect(
            &demo,
            &DetectOptions {
                min_score: 0.0,
                ..Default::default()
            },
        );
        assert_eq!(highlights.len(), 1);
        assert_eq!(highlights[0].end_tick, 1192);
        assert_eq!(highlights[0].key_moments, [[680, 1192]]);
    }

    #[test]
    fn round_tail_respects_official_end_next_round_and_demo_end() {
        for (official, next, demo_end, expected) in [
            (None, None, Some(1064), 1064),
            (None, None, Some(1000), 1000),
            (None, None, None, 1000),
            (Some(1200), None, Some(2280), 1200),
            (Some(1200), None, None, 1200),
            (None, Some(1100), Some(2280), 1100),
            (Some(2000), Some(1100), Some(2280), 1100),
            (Some(1200), None, Some(1064), 1064),
        ] {
            let mut events = vec![event("round_start", 100), event("round_end", 1000)];
            if let Some(tick) = official {
                events.push(event("round_officially_ended", tick));
            }
            if let Some(tick) = next {
                events.push(event("round_start", tick));
            }
            let mut groups: HashMap<&str, Vec<&GameEvent>> = HashMap::new();
            for ev in &events {
                groups.entry(&ev.name).or_default().push(ev);
            }
            let (rounds, _) = rounds_from_events(&groups, demo_end);
            assert_eq!(
                rounds[0].officially_ended_tick, expected,
                "official={official:?}, next={next:?}, demo_end={demo_end:?}"
            );
        }
    }
}

#[cfg(test)]
mod damage_tests {
    use super::*;
    use parser::second_pass::game_events::EventField;

    fn round(start: i32) -> RoundInfo {
        RoundInfo {
            round: start,
            start_tick: start,
            freeze_end_tick: start,
            end_tick: start + 80,
            officially_ended_tick: start + 90,
            winner: Some(Team::T),
            reason: String::new(),
            roster: BTreeMap::from([
                ("enemy".into(), Team::T),
                ("friend".into(), Team::Ct),
                ("victim".into(), Team::Ct),
            ]),
            bomb_planted_tick: None,
            bomb_defused_tick: None,
            bomb_defuser: None,
            bomb_exploded_tick: None,
        }
    }

    fn hurt(tick: i32, attacker: &str, raw: i32, after: i32, weapon: &str) -> GameEvent {
        let fields = vec![
            ("attacker_steamid", Variant::String(attacker.into())),
            ("user_steamid", Variant::String("victim".into())),
            (
                "attacker_team_num",
                Variant::I32(if attacker == "enemy" { 2 } else { 3 }),
            ),
            ("user_team_num", Variant::I32(3)),
            ("user_health", Variant::I32(100)), // Entity sample may lag the same tick's hits.
            ("health", Variant::I32(after)),
            ("dmg_health", Variant::I32(raw)),
            ("weapon", Variant::String(weapon.into())),
        ];
        GameEvent {
            name: "player_hurt".into(),
            tick,
            fields: fields
                .into_iter()
                .map(|(name, value)| EventField {
                    name: name.into(),
                    data: Some(value),
                })
                .collect(),
        }
    }

    #[test]
    fn key_damage_ticks_include_every_weapon_and_both_players_without_a_kill() {
        let events: Vec<_> = [
            "ak47",
            "hegrenade",
            "inferno",
            "taser",
            "knife",
            "new_weapon",
        ]
        .iter()
        .enumerate()
        .map(|(i, weapon)| hurt(10 + i as i32, "enemy", 1, 99 - i as i32, weapon))
        .collect();
        let (damage, ticks) = damage_from_events(&events.iter().collect::<Vec<_>>(), &[round(1)]);
        assert_eq!(damage["enemy"].total, 6);
        assert_eq!(ticks["enemy"], vec![10, 11, 12, 13, 14, 15]);
        assert_eq!(ticks["victim"], ticks["enemy"]);
        let excluded = [
            hurt(20, "friend", 10, 90, "taser"),
            hurt(21, "enemy", 0, 90, "inferno"),
        ];
        let (_, ticks) = damage_from_events(&excluded.iter().collect::<Vec<_>>(), &[round(1)]);
        assert!(ticks.is_empty());
    }

    #[test]
    fn lethal_overkill_and_same_tick_hits_count_only_remaining_hp() {
        let events = [
            hurt(10, "enemy", 21, 79, "hkp2000"),
            hurt(10, "enemy", 134, 0, "hkp2000"),
            hurt(10, "enemy", 134, 0, "hkp2000"),
        ];
        let (damage, _) = damage_from_events(&events.iter().collect::<Vec<_>>(), &[round(1)]);
        assert_eq!(damage["enemy"].total, 100);
        assert_eq!(damage["enemy"].utility, 0);
    }

    #[test]
    fn excluded_damage_still_reduces_hp_available_to_enemies() {
        let events = [
            hurt(10, "friend", 20, 80, "ak47"),
            hurt(11, "", 10, 70, "world"),
            hurt(12, "victim", 5, 65, "inferno"),
            hurt(13, "enemy", 200, 0, "inferno"),
        ];
        let (damage, _) = damage_from_events(&events.iter().collect::<Vec<_>>(), &[round(1)]);
        assert_eq!(damage.len(), 2);
        assert_eq!(damage["friend"].friendly, 20);
        assert_eq!(damage["friend"].total, 0);
        assert_eq!(damage["friend"].utility, 0);
        assert_eq!(damage["enemy"].friendly, 0);
        assert_eq!(damage["enemy"].total, 65);
        assert_eq!(damage["enemy"].utility, 65);
    }

    #[test]
    fn health_resets_each_round_and_roster_is_a_team_fallback() {
        let mut events = [
            hurt(0, "enemy", 100, 0, "ak47"),
            hurt(10, "enemy", 200, 0, "ak47"),
            hurt(110, "enemy", 200, 0, "ak47"),
        ];
        for event in &mut events {
            event.fields.retain(|f| !f.name.ends_with("team_num"));
        }
        let (damage, _) =
            damage_from_events(&events.iter().collect::<Vec<_>>(), &[round(1), round(101)]);
        assert_eq!(damage["enemy"].total, 200);
    }
}

fn activity_from_events(
    events: &[GameEvent],
    rounds: &[RoundInfo],
) -> BTreeMap<String, ActivityStats> {
    let mut out: BTreeMap<String, ActivityStats> = BTreeMap::new();
    for event in events {
        if !matches!(event.name.as_str(), "weapon_fire" | "player_blind") {
            continue;
        }
        let f = Fields(event);
        if f.bool("is_freeze_period")
            || !rounds
                .iter()
                .any(|r| f.tick() >= r.start_tick && f.tick() <= r.officially_ended_tick)
        {
            continue;
        }
        if event.name == "player_blind" {
            let id = f.str("attacker_steamid");
            let duration = f.num("blind_duration");
            // The demo can emit blinds for dead targets; these provide no combat effect.
            if id.is_empty()
                || duration <= 1.0
                || f.opt_num("user_health").is_some_and(|hp| hp <= 0.0)
            {
                continue;
            }
            let (at, vt) = (f.int("attacker_team_num"), f.int("user_team_num"));
            if ![2, 3].contains(&at) || ![2, 3].contains(&vt) {
                continue;
            }
            let s = out.entry(id).or_default();
            if at == vt {
                s.teammates_flashed += 1;
            } else {
                s.enemies_flashed += 1;
                s.enemy_blind_seconds += duration;
            }
            continue;
        }
        let id = f.str("user_steamid");
        if id.is_empty() {
            continue;
        }
        let weapon = f.str("weapon");
        let weapon = weapon.strip_prefix("weapon_").unwrap_or(&weapon);
        let s = out.entry(id).or_default();
        if UTILITY_WEAPONS.contains(&weapon) {
            match weapon {
                "flashbang" => s.flashes += 1,
                "smokegrenade" => s.smokes += 1,
                "hegrenade" => s.hes += 1,
                "molotov" | "incgrenade" => s.fires += 1,
                _ => {}
            }
        } else if !weapon.contains("knife")
            && !weapon.contains("bayonet")
            && weapon != "c4"
            && !UTILITY_WEAPONS.contains(&weapon)
        {
            s.shots += 1;
        }
    }
    out
}

#[cfg(test)]
mod activity_tests {
    use super::*;
    use parser::second_pass::game_events::EventField;
    #[test]
    fn counts_throws_and_qualifying_blinds_without_counting_grenades_as_shots() {
        let round: RoundInfo = serde_json::from_value(serde_json::json!({"round":1,"startTick":10,"freezeEndTick":10,"endTick":100,"officiallyEndedTick":110,"reason":"","roster":{}})).unwrap();
        let event = |name: &str, weapon: &str, duration: f32, team: i32, tick: i32| GameEvent {
            name: name.into(),
            tick,
            fields: vec![
                EventField {
                    name: "weapon".into(),
                    data: Some(Variant::String(weapon.into())),
                },
                EventField {
                    name: "user_steamid".into(),
                    data: Some(Variant::String("a".into())),
                },
                EventField {
                    name: "attacker_steamid".into(),
                    data: Some(Variant::String("a".into())),
                },
                EventField {
                    name: "attacker_team_num".into(),
                    data: Some(Variant::I32(3)),
                },
                EventField {
                    name: "user_team_num".into(),
                    data: Some(Variant::I32(team)),
                },
                EventField {
                    name: "blind_duration".into(),
                    data: Some(Variant::F32(duration)),
                },
            ],
        };
        let mut dead = event("player_blind", "", 4.2, 2, 70);
        dead.fields.push(EventField {
            name: "user_health".into(),
            data: Some(Variant::I32(0)),
        });
        let events = vec![
            event("weapon_fire", "weapon_flashbang", 0.0, 3, 20),
            event("weapon_fire", "weapon_ak47", 0.0, 3, 30),
            event("weapon_fire", "weapon_knife", 0.0, 3, 40),
            event("weapon_fire", "weapon_hegrenade", 0.0, 3, 50),
            event("player_blind", "", 3.0, 2, 60),
            event("player_blind", "", 0.5, 2, 60),
            event("player_blind", "", 2.0, 3, 60),
            event("weapon_fire", "weapon_ak47", 0.0, 3, 111),
            event("player_blind", "", 1.05, 2, 80),
            event("player_blind", "", 1.0, 2, 80),
            dead,
        ];
        let stats = activity_from_events(&events, &[round]);
        let a = &stats["a"];
        assert_eq!(
            (
                a.shots,
                a.flashes,
                a.hes,
                a.enemies_flashed,
                a.teammates_flashed
            ),
            (1, 1, 1, 2, 1)
        );
        assert!((a.enemy_blind_seconds - 4.05).abs() < 0.0001);
    }
}

#[cfg(test)]
mod vector_rows_tests {
    use super::*;
    #[test]
    fn position_vectors_preserve_fractional_values_and_missing_samples() {
        let column = VarVec::XYZVec(vec![Some([1.25, -2.5, 3.75]), None]);
        assert_eq!(col_len(&column), 2);
        let rows = Rows {
            cols: vec![column],
            index: HashMap::from([("position".into(), 0)]),
            order: vec![0, 1],
        };
        let values: Vec<_> = rows.iter().map(|r| r.vec3("position")).collect();
        assert_eq!(values, vec![Some([1.25, -2.5, 3.75]), None]);
        assert_eq!(rows.iter().next().unwrap().vec3("absent"), None);
    }
}

fn annotate_smoke_policy(
    events: &mut [GameEvent],
    policies: Vec<crate::analysis::smoke::spread::FirePolicy>,
) {
    use parser::second_pass::game_events::EventField;
    let policies: BTreeMap<_, _> = policies
        .into_iter()
        .map(|p| {
            (
                (p.tick, p.player, p.seed, p.item_def_index),
                (p.policy, p.rewind),
            )
        })
        .collect();
    for event in events.iter_mut().filter(|e| e.name == "fire_bullets") {
        let unsigned = |name: &str| {
            event.fields.iter().find_map(|f| {
                if f.name != name {
                    return None;
                }
                match f.data {
                    Some(Variant::U32(v)) => Some(v),
                    _ => None,
                }
            })
        };
        let (Some(player), Some(seed), Some(item)) = (
            unsigned("player"),
            unsigned("seed"),
            unsigned("item_def_index"),
        ) else {
            continue;
        };
        let Some((policy, rewind)) = policies.get(&(event.tick, player, seed, item)) else {
            continue;
        };
        if let Some(rewind) = rewind {
            event.fields.push(EventField {
                name: "rewind_max_unlag".into(),
                data: Some(Variant::F32(rewind.max_unlag)),
            });
            for (name, value) in [
                ("rewind_use_full_interp", rewind.use_full_interp),
                ("rewind_force_full_interp", rewind.force_full_interp),
                ("rewind_force_target_time", rewind.force_target_time),
                (
                    "rewind_filter_player_simulation_time",
                    rewind.filter_player_simulation_time,
                ),
                ("rewind_command_queue_enabled", rewind.command_queue_enabled),
            ] {
                event.fields.push(EventField {
                    name: name.into(),
                    data: Some(Variant::Bool(value)),
                });
            }
        }
        for (name, value) in [
            ("smoke_patterns_enabled", policy.patterns_enabled),
            ("smoke_only_up", policy.only_up),
            ("smoke_maximum_inaccuracy", policy.maximum_inaccuracy),
        ] {
            event.fields.push(EventField {
                name: name.into(),
                data: Some(Variant::Bool(value)),
            });
        }
    }
}
#[cfg(test)]
mod smoke_policy_tests {
    use super::*;
    use crate::analysis::smoke::spread::{FirePolicy, Policy};
    use parser::second_pass::game_events::EventField;
    #[test]
    fn annotation_requires_the_same_wire_shot() {
        let event = |seed| GameEvent {
            name: "fire_bullets".into(),
            tick: 10,
            fields: [("player", 2), ("seed", seed), ("item_def_index", 7)]
                .into_iter()
                .map(|(name, v)| EventField {
                    name: name.into(),
                    data: Some(Variant::U32(v)),
                })
                .collect(),
        };
        let mut events = [event(3), event(4)];
        annotate_smoke_policy(
            &mut events,
            vec![FirePolicy {
                rewind: Some(crate::analysis::ballistics::rewind::ReplicatedPolicy {
                    max_unlag: 0.5,
                    use_full_interp: true,
                    force_full_interp: false,
                    force_target_time: false,
                    filter_player_simulation_time: true,
                    command_queue_enabled: true,
                }),
                tick: 10,
                player: 2,
                seed: 3,
                item_def_index: 7,
                policy: Policy {
                    patterns_enabled: true,
                    only_up: false,
                    maximum_inaccuracy: false,
                },
            }],
        );
        let first = serde_json::to_value(&events[0]).unwrap();
        assert_eq!(first["rewind_max_unlag"], 0.5);
        assert_eq!(first["rewind_use_full_interp"], true);
        assert_eq!(first["rewind_filter_player_simulation_time"], true);
        assert_eq!(first["rewind_command_queue_enabled"], true);
        assert_eq!(first["smoke_patterns_enabled"], true);
        assert_eq!(first["smoke_only_up"], false);
        assert_eq!(first["smoke_maximum_inaccuracy"], false);
        assert!(serde_json::to_value(&events[1])
            .unwrap()
            .get("smoke_patterns_enabled")
            .is_none());
    }
}
