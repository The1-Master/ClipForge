//! Copy the complete Steam video profile into the owned USRLOCALCSGO profile.
//! Complete-profile preservation follows CSDM's video-config-file.ts; recording
//! uses its ordinary Windowed launch route (MIT upstream pin
//! b45a5f29283590c3fd0fc6526564b49567c0b931). Steam is read-only.

use anyhow::{anyhow, bail, ensure, Context, Result};
use std::{collections::BTreeMap, fs, io::{Read, Write}, path::{Path, PathBuf}};

const MAX_CONFIG: u64 = 64 * 1024;
const MAX_ACCOUNTS: usize = 1024;

#[derive(Debug)]
enum Token {
    Text { start: usize, end: usize, value: String },
    Open,
    Close { start: usize },
}

fn tokens(text: &str) -> Result<Vec<Token>> {
    let bytes = text.as_bytes();
    let mut i = if text.starts_with('\u{feff}') { 3 } else { 0 };
    let mut out = Vec::new();
    while i < bytes.len() {
        match bytes[i] {
            b if b.is_ascii_whitespace() => i += 1,
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' { i += 1; }
            }
            b'{' => { out.push(Token::Open); i += 1; }
            b'}' => { out.push(Token::Close { start: i }); i += 1; }
            b'"' => {
                let start = i;
                i += 1;
                let mut value = String::new();
                let mut segment = i;
                loop {
                    ensure!(i < bytes.len(), "Unterminated video profile string");
                    if bytes[i] == b'"' {
                        value.push_str(&text[segment..i]);
                        i += 1;
                        break;
                    }
                    if bytes[i] == b'\\' {
                        value.push_str(&text[segment..i]);
                        i += 1;
                        ensure!(i < bytes.len(), "Unterminated video profile escape");
                        // VDF keys used here are ASCII; preserve all unknown raw bytes.
                        if matches!(bytes[i], b'"' | b'\\') {
                            value.push(bytes[i] as char);
                            i += 1;
                        } else {
                            value.push('\\');
                        }
                        segment = i;
                    } else { i += 1; }
                }
                out.push(Token::Text { start, end: i, value });
            }
            _ => bail!("Video profile contains unsupported unquoted VDF data"),
        }
    }
    Ok(out)
}

fn profile_fields(text: &str) -> Result<(BTreeMap<String, (usize, usize, String)>, usize)> {
    let list = tokens(text)?;
    ensure!(matches!(list.first(), Some(Token::Text { value, .. }) if value == "video.cfg")
        && matches!(list.get(1), Some(Token::Open)), "Expected a complete video.cfg profile");
    let mut fields = BTreeMap::new();
    let mut depth = 1usize;
    let mut i = 2;
    let closing;
    loop {
        ensure!(i < list.len(), "Unclosed video profile block");
        match &list[i] {
            Token::Close { start } => {
                depth -= 1;
                if depth == 0 {
                    ensure!(i + 1 == list.len(), "Unexpected data after video profile");
                    closing = *start;
                    break;
                }
                i += 1;
            }
            Token::Text { value: key, .. } => {
                match list.get(i + 1) {
                    Some(Token::Text { start, end, value }) => {
                        if depth == 1 {
                            ensure!(fields.insert(key.clone(), (*start, *end, value.clone())).is_none(),
                                "Duplicate video profile setting");
                        }
                        i += 2;
                    }
                    Some(Token::Open) => { depth += 1; i += 2; }
                    _ => bail!("Video profile setting has no value"),
                }
            }
            Token::Open => bail!("Video profile block has no key"),
        }
    }
    // These hardware/autoconfiguration fields distinguish an actual complete
    // Steam profile from a synthesized resolution-only fragment.
    for required in ["Version", "VendorID", "DeviceID", "setting.cpu_level", "setting.gpu_level", "setting.gpu_mem_level"] {
        ensure!(fields.get(required).is_some_and(|(_, _, v)| v.parse::<u32>().is_ok()),
            "Steam video profile is incomplete: missing or invalid {required}");
    }
    Ok((fields, closing))
}

fn recording_profile(text: &str, width: u32, height: u32) -> Result<String> {
    ensure!((320..=7680).contains(&width) && (240..=4320).contains(&height), "Unsupported capture dimensions");
    let (fields, closing) = profile_fields(text)?;
    let values = [
        ("setting.fullscreen", "0".to_owned()),
        ("setting.nowindowborder", "0".to_owned()),
        ("setting.coop_fullscreen", "0".to_owned()),
        ("setting.defaultres", width.to_string()),
        ("setting.defaultresheight", height.to_string()),
    ];
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut changes = Vec::new();
    let mut additions = String::new();
    for (key, value) in values {
        if let Some((start, end, _)) = fields.get(key) {
            changes.push((*start, *end, format!("\"{value}\"")));
        } else {
            additions.push_str(&format!("\t\"{key}\"\t\"{value}\"{newline}"));
        }
    }
    if !additions.is_empty() { changes.push((closing, closing, additions)); }
    changes.sort_by_key(|(start, _, _)| std::cmp::Reverse(*start));
    let mut result = text.to_owned();
    for (start, end, value) in changes { result.replace_range(start..end, &value); }
    profile_fields(&result)?;
    Ok(result)
}

fn newest_baseline(steam: &Path) -> Result<String> {
    let userdata = steam.join("userdata").canonicalize()
        .context("CS2 needs a complete Steam video profile; open CS2 once to create video settings")?;
    let mut candidates = Vec::new();
    for (count, entry) in fs::read_dir(&userdata)?.enumerate() {
        ensure!(count < MAX_ACCOUNTS, "Steam userdata account enumeration exceeds its bound");
        let entry = entry?;
        let name = entry.file_name();
        if !name.to_string_lossy().bytes().all(|c| c.is_ascii_digit()) || name.is_empty() { continue; }
        let file = entry.path().join("730/local/cfg/cs2_video.txt");
        if !file.is_file() { continue; }
        let resolved = file.canonicalize()?;
        ensure!(resolved.starts_with(&userdata), "Steam video profile resolves outside userdata");
        let metadata = fs::metadata(&resolved)?;
        ensure!(metadata.len() <= MAX_CONFIG, "Steam video profile exceeds 64 KiB");
        candidates.push((metadata.modified()?, resolved));
    }
    candidates.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, path) in candidates {
        let mut bytes = Vec::new();
        fs::File::open(&path)?.take(MAX_CONFIG + 1).read_to_end(&mut bytes)?;
        ensure!(bytes.len() as u64 <= MAX_CONFIG, "Steam video profile grew beyond 64 KiB");
        let Ok(text) = String::from_utf8(bytes) else { continue; };
        if profile_fields(&text).is_ok() { return Ok(text); }
    }
    Err(anyhow!("No complete valid Steam cs2_video.txt was found; open CS2 once and save video settings"))
}

pub(super) fn prepare_owned_profile(profile: &Path, steam: &Path, width: u32, height: u32) -> Result<PathBuf> {
    let text = newest_baseline(steam)?;
    let updated = recording_profile(&text, width, height)?;
    let allowed = PathBuf::from(std::env::var_os("CLIPFORGE_WORKSPACE")
        .unwrap_or_else(|| "J:/CS2ClipStudio".into())).canonicalize()?;
    ensure!(profile.is_absolute() && !profile.components().any(|c| matches!(c, std::path::Component::ParentDir)),
        "Recording profile must be an absolute project path without parent traversal");
    let ancestor = profile.ancestors().find(|p| p.exists()).ok_or_else(|| anyhow!("Recording profile has no existing parent"))?;
    ensure!(ancestor.canonicalize()?.starts_with(&allowed), "Recording profile parent must stay in the owned workspace");
    fs::create_dir_all(profile)?;
    ensure!(profile.canonicalize()?.starts_with(&allowed), "Recording profile must stay in the owned workspace");
    let cfg = profile.join("cfg");
    fs::create_dir_all(&cfg)?;
    ensure!(cfg.canonicalize()?.starts_with(&allowed), "Recording cfg directory leaves the project");
    let target = cfg.join("cs2_video.txt");
    if target.exists() {
        ensure!(!fs::symlink_metadata(&target)?.file_type().is_symlink()
            && target.canonicalize()?.starts_with(&cfg.canonicalize()?), "Recording video profile is redirected");
    }
    let mut temp = tempfile::Builder::new().prefix("video-profile-").tempfile_in(&cfg)?;
    temp.write_all(updated.as_bytes())?;
    temp.as_file().sync_all()?;
    temp.persist(&target).map_err(|e| e.error)?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "\u{feff}\"video.cfg\"\r\n{\r\n\t\"Version\" \"16\"\r\n\t\"VendorID\" \"4098\"\r\n\t\"DeviceID\" \"301\"\r\n\t\"setting.cpu_level\" \"3\"\r\n\t\"setting.gpu_level\" \"3\"\r\n\t\"setting.gpu_mem_level\" \"3\"\r\n\t\"setting.defaultres\" \"1920\" // keep comment\r\n\t\"setting.defaultresheight\" \"1080\"\r\n\t\"unknown\" \"escaped\\\"value / 汉字\"\r\n\t\"nested\" { \"other\" \"kept\" }\r\n}\r\n";

    #[test]
    fn complete_profile_preserves_unknown_bytes_and_only_updates_display_settings() {
        let changed = recording_profile(BASE, 3840, 2160).unwrap();
        assert!(changed.contains("\"unknown\" \"escaped\\\"value / 汉字\""));
        assert!(changed.contains("\"nested\" { \"other\" \"kept\" }"));
        assert!(changed.contains("\"3840\" // keep comment\r\n"));
        let (fields, _) = profile_fields(&changed).unwrap();
        for (key, expected) in [("setting.defaultres", "3840"), ("setting.defaultresheight", "2160"),
            ("setting.fullscreen", "0"), ("setting.nowindowborder", "0"), ("setting.coop_fullscreen", "0")] {
            assert_eq!(fields[key].2, expected);
        }
        assert_eq!(fields["Version"].2, "16");
        assert_eq!(recording_profile(&changed, 3840, 2160).unwrap(), changed);
    }

    #[test]
    fn partial_malformed_or_duplicate_video_profiles_are_rejected() {
        assert!(recording_profile("\"video.cfg\" { \"setting.defaultres\" \"3840\" }", 3840, 2160).is_err());
        assert!(recording_profile(&BASE.replace("\"16\"", "\"invalid\""), 3840, 2160).is_err());
        assert!(recording_profile(&BASE.replace("// keep comment", "\"Version\" \"17\""), 3840, 2160).is_err());
        assert!(recording_profile(&BASE.replace('}', ""), 3840, 2160).is_err());
        assert!(recording_profile(BASE, 0, 2160).is_err());
    }

    #[test]
    fn copying_a_baseline_does_not_modify_steam_and_writes_only_owned_cfg() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("steam/userdata/123/730/local/cfg");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("cs2_video.txt"), BASE).unwrap();
        let target = prepare_owned_profile(&dir.path().join("profile"), &dir.path().join("steam"), 3840, 2160).unwrap();
        assert_eq!(target, dir.path().join("profile/cfg/cs2_video.txt"));
        assert_eq!(fs::read_to_string(source.join("cs2_video.txt")).unwrap(), BASE);
        assert_eq!(profile_fields(&fs::read_to_string(target).unwrap()).unwrap().0["setting.defaultres"].2, "3840");
    }
}
