//! Data-first, permissive discovery. Candidates describe facts; visual review
//! decides whether missed damage really was an easy-shot whiff or a blooper.
use crate::model::*;
use crate::stats::ParsedDemo;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

fn identity(source: &str, player: &str, kind: &str, start: i32, end: i32) -> String {
    format!("candidate-{:x}", Sha256::digest(format!("{source}:{player}:{kind}:{start}:{end}")))
}
fn evidence(event: &DiscoveryEvent) -> DiscoveryEvidence {
    DiscoveryEvidence { event_id: event.id.clone(), tick: event.tick, kind: event.kind.clone(),
        weapon: event.weapon.clone(), value: event.value, target_id: event.target_id.clone() }
}
fn weapon(name: &str) -> &str {
    match name.strip_prefix("weapon_").unwrap_or(name) { "usp_silencer" => "hkp2000", "m4a1_silencer" => "m4a1", other => other }
}
fn fired_weapon(name: &str) -> bool {
    !matches!(name, "flashbang" | "smokegrenade" | "hegrenade" | "molotov" | "incgrenade" | "decoy" | "c4" | "")
}
fn candidate(demo: &DemoData, player: &str, round: &RoundInfo, category: &str, title: String,
             anchor: i32, last: i32, facts: Vec<DiscoveryEvidence>, tags: Vec<String>, visual: bool) -> DiscoveryCandidate {
    let lead = seconds_to_ticks(5.0, demo.info.tick_rate);
    let tail = seconds_to_ticks(3.0, demo.info.tick_rate);
    let start = anchor.saturating_sub(lead).max(round.freeze_end_tick);
    let end = last.saturating_add(tail).min(round.officially_ended_tick).max(start + 1);
    DiscoveryCandidate { id: identity(&demo.demo_digest, player, category, anchor, last),
        source_digest: demo.demo_digest.clone(), player_id: player.to_string(), round: round.round,
        start_tick: start, end_tick: end, anchor_tick: anchor, category: category.into(), title,
        tags, origins: vec!["demo-events-v1".into()], evidence: facts, requires_visual_review: visual }
}

/// All actions are retained for targeted scouting. Missed fire is promoted to a
/// *possible* whiff when its weapon/count/context merits review, never proof.
pub fn discovery_candidates(demo: &DemoData, player: &str) -> Vec<DiscoveryCandidate> {
    let mut out = vec![];
    let gap = seconds_to_ticks(1.6, demo.info.tick_rate);
    let proximity = seconds_to_ticks(5.0, demo.info.tick_rate);
    for round in &demo.rounds {
        let events: Vec<_> = demo.event_ledger.iter().filter(|e| e.round == round.round).collect();
        let shots: Vec<_> = events.iter().copied().filter(|e| e.kind == "weapon_fire" && e.player_id == player && fired_weapon(&e.weapon)).collect();
        let mut cursor = 0;
        while cursor < shots.len() {
            let first = cursor;
            cursor += 1;
            while cursor < shots.len() && shots[cursor].tick - shots[cursor - 1].tick <= gap
                && weapon(&shots[cursor].weapon) == weapon(&shots[first].weapon)
                && shots[cursor].tick - shots[first].tick < seconds_to_ticks(20.0, demo.info.tick_rate) { cursor += 1; }
            let burst = &shots[first..cursor];
            let last = burst.last().unwrap().tick;
            let hit_count = burst.iter().filter(|shot| events.iter().any(|e| e.kind == "player_hurt"
                && e.player_id == player && e.relation == "enemy" && e.value.is_some_and(|v| v > 0.0)
                && e.tick >= shot.tick && e.tick <= shot.tick.saturating_add(1)
                && weapon(&e.weapon) == weapon(&shot.weapon))).count();
            let relevant = events.iter().copied().filter(|e| e.target_id.as_deref() == Some(player)
                && matches!(e.kind.as_str(), "player_hurt" | "player_death")
                && e.tick >= burst[0].tick && e.tick <= last.saturating_add(proximity)).collect::<Vec<_>>();
            let name = &burst[0].weapon;
            let risky = name == "taser" || name.starts_with("knife") || name.starts_with("bayonet");
            let precision = matches!(name.as_str(), "awp" | "ssg08" | "deagle" | "revolver");
            let possible_whiff = hit_count == 0 && (burst.len() >= 3 || risky || (precision && (burst.len() >= 2 || !relevant.is_empty())));
            let category = if possible_whiff { "whiff_candidate" } else { "combat" };
            let title = if possible_whiff { format!("Possible whiff — {} {} shot{} with no recorded enemy damage", burst.len(), name, if burst.len() == 1 { "" } else { "s" }) }
                else { format!("Review {} {} shot{}", burst.len(), name, if burst.len() == 1 { "" } else { "s" }) };
            let mut facts = burst.iter().map(|e| evidence(e)).collect::<Vec<_>>();
            facts.extend(relevant.into_iter().map(evidence));
            // Bound API payloads while keeping a long burst's setup and outcome.
            if facts.len() > 64 { facts = (0..64).map(|i| facts[i * (facts.len() - 1) / 63].clone()).collect(); }
            out.push(candidate(demo, player, round, category, title, burst[0].tick, last, facts,
                vec![category.into(), "visual-review-needed".into()], true));
        }
        for event in events.iter().copied() {
            if event.kind == "player_hurt" && event.player_id == player && matches!(event.relation.as_str(), "self" | "teammate")
                || event.kind == "player_blind" && event.player_id == player && matches!(event.relation.as_str(), "self" | "teammate") && event.value.is_some_and(|v| v >= 1.0) {
                out.push(candidate(demo, player, round, "utility_mistake", "Possible utility mistake — review the recorded team/self effect".into(), event.tick, event.tick,
                    vec![evidence(event)], vec!["utility_mistake".into()], true));
            }
            if event.kind == "player_hurt" && event.target_id.as_deref() == Some(player) && event.relation == "enemy" {
                out.push(candidate(demo, player, round, "combat", "Incoming damage — review the encounter".into(), event.tick, event.tick, vec![evidence(event)], vec!["combat".into()], true));
            }
            if matches!(event.kind.as_str(), "bomb_planted" | "bomb_defused") && event.player_id == player {
                out.push(candidate(demo, player, round, "tension", "Bomb objective — review the timing".into(), event.tick, event.tick, vec![evidence(event)], vec!["tension".into()], true));
            }
        }
        for kill in demo.kills.iter().filter(|k| k.tick >= round.freeze_end_tick && k.tick <= round.officially_ended_tick && !k.is_freeze_period) {
            if kill.attacker.as_ref().is_some_and(|a| a.steamid == player && a.team != kill.victim.team) {
                let tags = special_tags(kill);
                let exceptional = !tags.is_empty();
                let category = if exceptional { "exceptional_shot" } else { "combat" };
                let fact = events.iter().find(|e| e.kind == "player_death" && e.tick == kill.tick && e.target_id.as_deref() == Some(&kill.victim.steamid)).map(|e| evidence(e)).into_iter().collect();
                out.push(candidate(demo, player, round, category, if exceptional { format!("Exceptional shot — {}", tags.join(", ")) } else { "Enemy kill — review the play".into() }, kill.tick, kill.tick,
                    fact, if exceptional { tags } else { vec!["combat".into()] }, !exceptional));
            }
        }
    }
    for death in demo.recorded_deaths.iter().filter(|d| d.kill.victim.steamid == player) {
        let k = &death.kill;
        let round_number = death.round_number.unwrap_or(0);
        let observed_end = demo.demo_end_tick.max(demo.rounds.iter().map(|r| r.officially_ended_tick).max().unwrap_or(0)).max(k.tick);
        let end = k.tick.saturating_add(seconds_to_ticks(3.0, demo.info.tick_rate)).min(observed_end);
        out.push(DiscoveryCandidate { id: identity(&demo.demo_digest, player, "death", k.tick, k.tick), source_digest: demo.demo_digest.clone(),
            player_id: player.into(), round: round_number, start_tick: k.tick.saturating_sub(seconds_to_ticks(5.0, demo.info.tick_rate)).max(0), end_tick: end,
            anchor_tick: k.tick, category: "death".into(), title: format!("Personal death — {}", death.phase), tags: death.tags.clone(), origins: vec!["death-ledger-v1".into()],
            evidence: vec![DiscoveryEvidence { event_id: death.id.clone(), tick: k.tick, kind: "player_death".into(), weapon: k.weapon.clone(), value: None, target_id: Some(player.into()) }], requires_visual_review: false });
    }
    out.sort_by_key(|c| (c.start_tick, c.anchor_tick, c.id.clone()));
    out.dedup_by(|a, b| a.id == b.id);
    out
}

fn special_tags(k: &KillEvent) -> Vec<String> {
    let mut tags = vec![];
    for (condition, tag) in [(k.noscope && matches!(k.weapon.as_str(), "awp" | "ssg08" | "scar20" | "g3sg1"), "noscope"),
        (k.penetrated > 0, "wallbang"), (k.thru_smoke, "thrusmoke"), (k.attacker_blind, "blind"), (k.attacker_in_air, "airborne"),
        (k.weapon.starts_with("knife") || k.weapon.starts_with("bayonet"), "knife"), (k.weapon == "taser", "zeus"),
        (k.headshot && k.distance >= 35.0 && matches!(k.weapon.as_str(), "deagle" | "revolver" | "hkp2000" | "usp_silencer" | "glock" | "p250"), "long-range-headshot")] {
        if condition { tags.push(tag.to_string()); }
    }
    tags
}

pub fn exceptional_shot_highlights(demo: &DemoData, opts: &DetectOptions) -> Vec<Highlight> {
    demo.kills.iter().filter_map(|k| {
        let a = k.attacker.as_ref()?;
        if a.steamid.is_empty() || a.steamid == k.victim.steamid || a.team == k.victim.team || k.is_freeze_period
            || !opts.players.is_empty() && !opts.players.contains(&a.steamid) { return None; }
        let round = demo.rounds.iter().find(|r| k.tick >= r.freeze_end_tick && k.tick <= r.officially_ended_tick)?;
        let mut tags = special_tags(k);
        if tags.is_empty() { return None; }
        tags.push("exceptional-shot".into());
        let start_tick = k.tick.saturating_sub(seconds_to_ticks(opts.lead_in_seconds, demo.info.tick_rate).max(0)).max(round.freeze_end_tick);
        let end_tick = k.tick.saturating_add(seconds_to_ticks(opts.lead_out_seconds, demo.info.tick_rate).max(0)).min(round.officially_ended_tick);
        Some(Highlight { id: super::stable_moment_id(demo, "exceptional-shot", &a.steamid, k.tick, round.round), player: HighlightPlayer { steamid: a.steamid.clone(), name: a.name.clone() },
            round: round.round, start_tick, end_tick, anchor_tick: k.tick, key_moments: vec![[start_tick, end_tick]], round_result: None,
            score: 1.0, title: format!("{} — Exceptional shot · {} · R{}", a.name, tags[..tags.len() - 1].join(", "), round.round),
            tags, kills: vec![k.clone()], breakdown: BTreeMap::from([("exceptionalShot".into(), 1.0)]) })
    }).collect()
}

/// Chronological candidate batches reduce seek/launch overhead. Batches never
/// cross round boundaries or merge distant action into a full-round recording.
pub fn review_plan(parsed: &ParsedDemo, player: &str, mode: &str) -> Result<ReviewPlan, String> {
    if !matches!(mode, "targeted" | "full-sweep") { return Err("Choose targeted or full-sweep review.".into()); }
    if !parsed.info.players.iter().any(|p| p.steamid == player) { return Err("The selected player is not in this match.".into()); }
    if !parsed.info.tick_rate.is_finite() || parsed.info.tick_rate <= 0.0 { return Err("The match has no valid tick rate.".into()); }
    if mode == "targeted" && parsed.event_ledger_version != 1 { return Err("Parse this match again to build its fast discovery ledger.".into()); }
    let candidates = parsed.discovery_candidates.iter().filter(|c| c.player_id == player).cloned().collect::<Vec<_>>();
    let mut windows: Vec<ReviewWindow> = vec![];
    let max_batch = seconds_to_ticks(30.0, parsed.info.tick_rate);
    let join_gap = seconds_to_ticks(2.0, parsed.info.tick_rate);
    for round in &parsed.rounds {
        if round.officially_ended_tick <= round.freeze_end_tick { continue; }
        if mode == "full-sweep" { windows.push(ReviewWindow { round: round.round, start_tick: round.freeze_end_tick.max(0), end_tick: round.officially_ended_tick, candidate_ids: candidates.iter().filter(|c| c.round == round.round).map(|c| c.id.clone()).collect() }); continue; }
        for c in candidates.iter().filter(|c| c.round == round.round) {
            let start = c.start_tick.max(round.freeze_end_tick).max(0);
            let end = c.end_tick.min(round.officially_ended_tick);
            if end <= start { continue; }
            if let Some(prior) = windows.last_mut().filter(|w| w.round == round.round && start <= w.end_tick.saturating_add(join_gap)) {
                prior.end_tick = prior.end_tick.max(end);
                if !prior.candidate_ids.contains(&c.id) { prior.candidate_ids.push(c.id.clone()); }
            } else { windows.push(ReviewWindow { round: round.round, start_tick: start, end_tick: end, candidate_ids: vec![c.id.clone()] }); }
        }
    }
    if mode == "targeted" {
        let mut bounded = Vec::new();
        for window in windows {
            let mut start = window.start_tick;
            loop {
                let end = start.saturating_add(max_batch).min(window.end_tick);
                let candidate_ids = candidates.iter().filter(|c| c.round == window.round && c.start_tick <= end && c.end_tick >= start).map(|c| c.id.clone()).collect();
                bounded.push(ReviewWindow { round: window.round, start_tick: start, end_tick: end, candidate_ids });
                if end == window.end_tick { break; }
                // Explicit two-second context overlap protects boundary events;
                // independent candidates cannot accidentally repeat whole clips.
                start = end - join_gap;
            }
        }
        windows = bounded;
    }
    let full = parsed.rounds.iter().map(|r| (r.officially_ended_tick - r.freeze_end_tick).max(0) as f64 / parsed.info.tick_rate).sum();
    let preview = windows.iter().map(|w| (w.end_tick - w.start_tick) as f64 / parsed.info.tick_rate).sum();
    Ok(ReviewPlan { schema_version: 1, mode: mode.into(), coverage_scope: if mode == "targeted" { "event-candidates" } else { "competitive-rounds" }.into(), candidates, windows, preview_seconds: preview, full_match_seconds: full })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo() -> DemoData {
        serde_json::from_value(serde_json::json!({"demoDigest":"fixture-source","demoEndTick":16000,"eventLedgerVersion":1,
            "info":{"path":"fixture.dem","mapName":"de_fixture","serverName":"","tickRate":64,"players":[{"steamid":"player","name":"Player","teamNumber":3},{"steamid":"enemy","name":"Enemy","teamNumber":2}]},
            "kills":[],"rounds":[{"round":1,"startTick":0,"freezeEndTick":64,"endTick":7000,"officiallyEndedTick":7100,"reason":"","roster":{"player":"CT","enemy":"TERRORIST"}},{"round":2,"startTick":7200,"freezeEndTick":7300,"endTick":15900,"officiallyEndedTick":16000,"reason":"","roster":{"player":"CT","enemy":"TERRORIST"}}]})).unwrap()
    }
    fn event(tick: i32, kind: &str, name: &str, target: Option<&str>, weapon: &str, value: Option<f64>) -> DiscoveryEvent {
        DiscoveryEvent { id: format!("{kind}-{tick}"), tick, round: if tick < 7200 { 1 } else { 2 }, kind: kind.into(), player_id: name.into(), target_id: target.map(str::to_string), weapon: weapon.into(), value,
            relation: if target.is_some() { "enemy" } else { "none" }.into() }
    }
    #[test]
    fn missed_bursts_are_possible_whiffs_not_proof_and_hits_or_utility_do_not_become_whiffs() {
        let mut d = demo();
        d.event_ledger = vec![event(1000,"weapon_fire","player",None,"ak47",None),event(1007,"weapon_fire","player",None,"ak47",None),event(1014,"weapon_fire","player",None,"ak47",None)];
        let c = discovery_candidates(&d,"player");
        assert_eq!(c.len(),1); assert_eq!(c[0].category,"whiff_candidate"); assert!(c[0].title.starts_with("Possible whiff")); assert!(c[0].requires_visual_review);
        assert_eq!(c[0].evidence.len(),3); assert_eq!(c[0].source_digest,"fixture-source");
        d.event_ledger.push(event(1007,"player_hurt","player",Some("enemy"),"ak47",Some(1.0)));
        assert!(discovery_candidates(&d,"player").iter().all(|c| c.category != "whiff_candidate"));
        for e in &mut d.event_ledger { e.weapon="smokegrenade".into(); }
        assert!(discovery_candidates(&d,"player").is_empty());
        assert!(discovery_candidates(&d,"enemy").iter().all(|c| c.category != "whiff_candidate"));
    }
    #[test]
    fn single_precision_miss_followed_by_death_and_failed_taser_are_retained() {
        let mut d = demo();
        d.event_ledger = vec![event(1000,"weapon_fire","player",None,"awp",None),event(1050,"player_death","enemy",Some("player"),"ak47",None),event(2000,"weapon_fire","player",None,"taser",None)];
        let c = discovery_candidates(&d,"player");
        assert_eq!(c.iter().filter(|c| c.category == "whiff_candidate").count(),2);
        assert!(c.iter().any(|c| c.evidence.iter().any(|e| e.kind=="player_death")));
    }
    #[test]
    fn rare_single_shot_bypasses_round_score_and_cap_without_friendly_kills() {
        let mut d = demo();
        let kill: KillEvent=serde_json::from_value(serde_json::json!({"tick":1000,"round":0,"attacker":{"steamid":"player","name":"Player","team":"CT","health":100,"weaponName":"awp"},"victim":{"steamid":"enemy","name":"Enemy","team":"TERRORIST","weaponName":"ak47"},"weapon":"awp","headshot":false,"noscope":true,"penetrated":0,"thruSmoke":false,"attackerBlind":false,"attackerInAir":false,"assistedFlash":false,"distance":10,"hitgroup":"chest","isFreezePeriod":false})).unwrap();
        d.kills=vec![kill.clone()];
        let opts=DetectOptions{min_score:999.0,top_n:1,include_deaths:false,..Default::default()};
        let highlights=super::super::detect(&d,&opts);
        assert_eq!(highlights.len(),1); assert!(highlights[0].tags.iter().any(|t|t=="exceptional-shot")); assert_eq!(highlights[0].kills.len(),1);
        assert!(discovery_candidates(&d,"player").iter().any(|c|c.category=="exceptional_shot"));
        d.kills[0].victim.team=Team::Ct; assert!(super::super::detect(&d,&opts).is_empty());
    }
    #[test]
    fn review_batches_are_chronological_bounded_and_leave_non_event_time_outside_targeted_scope() {
        let mut d=demo();
        d.event_ledger=vec![event(1000,"weapon_fire","player",None,"awp",None),event(1400,"weapon_fire","player",None,"awp",None),event(8000,"weapon_fire","player",None,"awp",None)];
        let parsed=crate::stats::build_parsed_demo(d);
        let fast=review_plan(&parsed,"player","targeted").unwrap();
        let full=review_plan(&parsed,"player","full-sweep").unwrap();
        assert!(fast.preview_seconds < full.preview_seconds / 2.0); assert_eq!(fast.coverage_scope,"event-candidates");
        assert_eq!(fast.windows.len(),2); assert_eq!(fast.windows[0].candidate_ids.len(),2);
        assert!(fast.windows.iter().all(|w|w.end_tick-w.start_tick<=64*30));
        assert!(fast.windows.windows(2).all(|pair|pair[0].end_tick<pair[1].start_tick));
        let mut legacy=parsed;legacy.event_ledger_version=0;
        assert!(review_plan(&legacy,"player","targeted").is_err()); assert!(review_plan(&legacy,"player","full-sweep").is_ok());
        assert!(review_plan(&legacy,"other","full-sweep").is_err()); assert!(review_plan(&legacy,"player","4x").is_err());
    }
    #[test]
    fn candidate_identity_survives_rename_but_changes_with_source_or_pov() {
        let mut d=demo(); d.event_ledger=vec![event(1000,"weapon_fire","player",None,"taser",None)];
        let original=discovery_candidates(&d,"player")[0].id.clone();d.info.path="renamed.dem".into();
        assert_eq!(original,discovery_candidates(&d,"player")[0].id);
        d.demo_digest="changed-source".into();assert_ne!(original,discovery_candidates(&d,"player")[0].id);
        assert_ne!(identity("s","p1","combat",1,2),identity("s","p2","combat",1,2));
    }
}
