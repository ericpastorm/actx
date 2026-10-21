//! Paths: showing them, Claude Code's folder names, and rewriting another machine's paths.
//! Paths cross between Linux, macOS and Windows machines, so they're stored with `/` and
//! rewritten between the two flavors (`/home/me/shop` ⇄ `C:\Users\me\shop`).

use std::path::{Path, PathBuf};

use crate::ctx::ctx;

/// A path as this machine writes it: on Windows, `\` separators and no `\\?\` prefix.
pub fn native(path: &str) -> String {
    if cfg!(windows) {
        windows_native(path)
    } else {
        path.to_string()
    }
}

fn windows_native(path: &str) -> String {
    let path = path.replace('/', "\\");
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else {
        path.strip_prefix(r"\\?\").unwrap_or(&path).to_string()
    }
}

/// `std::fs::canonicalize` without the `\\?\` prefix Windows adds.
pub fn canonical(path: &Path) -> std::io::Result<PathBuf> {
    let p = std::fs::canonicalize(path)?;
    Ok(if cfg!(windows) {
        PathBuf::from(native(&p.to_string_lossy()))
    } else {
        p
    })
}

/// Make a file readable by this user only (Windows keeps the profile folder's permissions).
pub fn make_private(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

/// Make a file runnable (on Windows that's the .exe extension).
pub fn make_executable(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

/// Same path with `/` as the separator.
pub fn slashed(path: &str) -> String {
    path.replace('\\', "/")
}

pub fn short(path: &str) -> String {
    let home = ctx().home_str();
    let home = home.trim_end_matches(['/', '\\']);
    match path.strip_prefix(home) {
        Some("") => "~".to_string(),
        Some(rest) if rest.starts_with(['/', '\\']) => format!("~{rest}"),
        _ => path.to_string(),
    }
}

/// The path relative to $HOME ("." for $HOME itself), if it's inside it. Always with `/`.
pub fn rel_home(path: &str) -> Option<String> {
    let rel = Path::new(path).strip_prefix(&ctx().home).ok()?;
    let rel = slashed(&rel.to_string_lossy());
    Some(if rel.is_empty() { ".".to_string() } else { rel })
}

/// $HOME/rel, the way it was stored.
pub fn under_home(rel: &str) -> String {
    let mut p = ctx().home.clone();
    if rel != "." {
        p.extend(rel.split('/').filter(|c| !c.is_empty()));
    }
    p.to_string_lossy().into_owned()
}

/// How Claude Code names a project's folder in ~/.claude/projects.
pub fn encode_claude(path: &str) -> String {
    path.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Undo encode_claude by looking at the disk ('-' may come from '/', '-', '_', '.', ' '...).
pub fn resolve_encoded(name: &str) -> Option<String> {
    fn walk(base: &Path, rest: &str) -> Option<String> {
        if rest.is_empty() {
            return Some(base.to_string_lossy().into_owned());
        }
        let mut entries: Vec<String> = std::fs::read_dir(base)
            .ok()?
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect();
        entries.sort();
        for e in entries {
            let enc = format!("-{}", encode_claude(&e));
            if (rest == enc || rest.starts_with(&format!("{enc}-")))
                && let Some(found) = walk(&base.join(&e), &rest[enc.len()..])
            {
                return Some(found);
            }
        }
        None
    }
    if name.starts_with('-') {
        walk(Path::new("/"), name)
    } else if cfg!(windows)
        && let [drive, b'-', b'-', ..] = name.as_bytes()
        && drive.is_ascii_alphabetic()
    {
        // C:\Users\me → "C--Users-me": the drive, then a '-' for the colon and one per separator
        walk(Path::new(&format!("{}:\\", *drive as char)), &name[2..])
    } else {
        None
    }
}

/// The last name in a path from any machine (`/home/me/shop` or `C:\Users\me\shop`).
pub fn base_name(path: &str) -> &str {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
}

/// Is this a Windows path (`C:\…`, `C:/…` or `\\server\…`)?
pub fn is_windows_path(path: &str) -> bool {
    let b = path.as_bytes();
    (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/'))
        || path.starts_with(r"\\")
}

/// One way a path can be written in a conversation, and what to put instead.
struct Rule {
    old: Vec<u8>,
    /// separators that can follow it inside the same path
    seps_in: Vec<Vec<u8>>,
    new: Vec<u8>,
    sep_out: Vec<u8>,
    /// the separators differ: the rest of the path has to be converted too
    convert: bool,
}

/// Swaps another machine's paths for this one's inside a conversation (JSON text: a Windows path
/// is written with doubled backslashes). A path only matches as a whole name: /home/a/shop
/// matches /home/a/shop and /home/a/shop/x, not /home/a/shop-old. Between Linux and Windows the
/// rest of the path comes along with its separators changed.
pub struct PathFixer {
    rules: Vec<Rule>,
}

fn rules_for(old: &str, new: &str) -> Vec<Rule> {
    let new_win = is_windows_path(new);
    let escaped = |s: &str| s.replace('\\', "\\\\");
    // how the new path is written for each way the old one can be
    let (new_json, sep_json): (String, &str) = if new_win {
        (escaped(&new.replace('/', "\\")), "\\\\")
    } else {
        (new.to_string(), "/")
    };
    let (new_slash, sep_slash) = (
        if new_win {
            new.replace('\\', "/")
        } else {
            new.to_string()
        },
        "/",
    );
    let rule = |old: String, seps: &[&str], new: &str, sep: &str| Rule {
        convert: seps != [sep],
        old: old.into_bytes(),
        seps_in: seps.iter().map(|s| s.as_bytes().to_vec()).collect(),
        new: new.as_bytes().to_vec(),
        sep_out: sep.as_bytes().to_vec(),
    };
    if is_windows_path(old) {
        let backslashed = old.replace('/', "\\");
        let mut rules = vec![
            rule(escaped(&backslashed), &["\\\\"], &new_json, sep_json),
            rule(backslashed.replace('\\', "/"), &["/"], &new_slash, sep_slash),
        ];
        if backslashed != old && old.contains('\\') {
            rules.push(rule(escaped(old), &["\\\\", "/"], &new_json, sep_json));
        }
        rules
    } else {
        vec![rule(old.to_string(), &["/"], &new_json, sep_json)]
    }
}

/// Copy a path suffix up to a quote, JSON escape, or reserved path character. Spaces and punctuation
/// can be part of a name; a separator after whitespace starts another path in the surrounding text.
fn convert_suffix(rule: &Rule, data: &[u8], mut end: usize, out: &mut Vec<u8>) -> usize {
    if !rule.seps_in.iter().any(|s| data[end..].starts_with(s)) {
        return end;
    }
    let start = end;
    while let Some(&c) = data.get(end) {
        if let Some(sep) = rule.seps_in.iter().find(|s| data[end..].starts_with(s)) {
            if end > start && data[end - 1].is_ascii_whitespace() {
                break;
            }
            out.extend_from_slice(&rule.sep_out);
            end += sep.len();
        } else if c.is_ascii_control() || b"\\\"<>|?*:".contains(&c) {
            break;
        } else {
            out.push(c);
            end += 1;
        }
    }
    end
}

impl PathFixer {
    pub fn new(pairs: &[(&str, &str)]) -> Option<PathFixer> {
        let rules: Vec<Rule> = pairs
            .iter()
            .filter(|(old, new)| !old.is_empty() && !new.is_empty() && old != new)
            .flat_map(|(old, new)| rules_for(old, new))
            .collect();
        if rules.is_empty() {
            None
        } else {
            Some(PathFixer { rules })
        }
    }

    pub fn fix(&self, data: &[u8]) -> Vec<u8> {
        let mut data = data.to_vec();
        for rule in &self.rules {
            let finder = memchr::memmem::Finder::new(&rule.old);
            if finder.find(&data).is_none() {
                continue;
            }
            let mut out = Vec::with_capacity(data.len());
            let mut pos = 0;
            for at in finder.find_iter(&data) {
                if at < pos {
                    continue;
                }
                let mut end = at + rule.old.len();
                let next = data.get(end).copied();
                let whole =
                    !matches!(next, Some(c) if c.is_ascii_alphanumeric() || c == b'_' || c == b'.' || c == b'-');
                if !whole {
                    continue;
                }
                out.extend_from_slice(&data[pos..at]);
                out.extend_from_slice(&rule.new);
                if rule.convert {
                    end = convert_suffix(rule, &data, end, &mut out);
                }
                pos = end;
            }
            out.extend_from_slice(&data[pos..]);
            data = out;
        }
        data
    }
}

pub type Fix<'a> = Option<&'a PathFixer>;

pub fn apply(fix: Fix, data: &[u8]) -> Vec<u8> {
    match fix {
        Some(f) => f.fix(data),
        None => data.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_fixer_respects_name_boundaries() {
        let fix = PathFixer::new(&[("/home/a/shop", "/home/b/work/shop2")]).unwrap();
        let data = b"\"cwd\":\"/home/a/shop\" edited /home/a/shop/cart.ts not /home/a/shop-old or /home/a/shopping";
        assert_eq!(
            fix.fix(data),
            b"\"cwd\":\"/home/b/work/shop2\" edited /home/b/work/shop2/cart.ts not /home/a/shop-old or /home/a/shopping"
        );
        assert!(PathFixer::new(&[("/same", "/same")]).is_none());
    }

    #[test]
    fn path_fixer_converts_between_linux_and_windows() {
        let data = br#"{"cwd":"/home/jose/shop","file":"/home/jose/shop/src/cart.ts","other":"/home/jose/shop-old/x"}"#;
        let fix = PathFixer::new(&[("/home/jose/shop", r"C:\Users\jmgam\work\shop")]).unwrap();
        let there = fix.fix(data);
        assert_eq!(
            String::from_utf8_lossy(&there),
            r#"{"cwd":"C:\\Users\\jmgam\\work\\shop","file":"C:\\Users\\jmgam\\work\\shop\\src\\cart.ts","other":"/home/jose/shop-old/x"}"#
        );
        let back = PathFixer::new(&[(r"C:\Users\jmgam\work\shop", "/home/jose/shop")]).unwrap();
        let again = back.fix(&there);
        assert_eq!(
            String::from_utf8_lossy(&again),
            r#"{"cwd":"/home/jose/shop","file":"/home/jose/shop/src/cart.ts","other":"/home/jose/shop-old/x"}"#
        );
        // Windows to Windows: another user and folder; a path ending the string or followed by \n
        let win = PathFixer::new(&[(r"C:\Users\a\shop", r"D:\p\shop")]).unwrap();
        assert_eq!(
            String::from_utf8_lossy(&win.fix(br#""C:\\Users\\a\\shop\\x.rs\nC:\\Users\\a\\shop""#)),
            r#""D:\\p\\shop\\x.rs\nD:\\p\\shop""#
        );
    }

    #[test]
    fn windows_path_detection_and_names() {
        assert!(is_windows_path(r"C:\Users\me"));
        assert!(is_windows_path("c:/Users/me"));
        assert!(!is_windows_path("/home/me"));
        assert_eq!(base_name(r"C:\Users\me\shop"), "shop");
        assert_eq!(base_name("/home/me/shop/"), "shop");
    }

    #[test]
    fn windows_native_preserves_network_roots() {
        for (input, expected) in [
            (r"\\?\UNC\nas\share\actx", r"\\nas\share\actx"),
            (r"\\nas\share\actx", r"\\nas\share\actx"),
            (r"\\?\C:\Users\me\shop", r"C:\Users\me\shop"),
            ("C:/Users/me/shop", r"C:\Users\me\shop"),
        ] {
            assert_eq!(windows_native(input), expected, "{input}");
        }
    }

    #[test]
    fn path_fixer_keeps_complete_names_and_trailing_separators() {
        let to_unix = PathFixer::new(&[(r"C:\Users\me\shop", "/home/me/shop")]).unwrap();
        let to_windows = PathFixer::new(&[("/home/me/shop", r"C:\Users\me\shop")]).unwrap();
        for suffix in [
            "my folder/cart.ts",
            "a,b/ñ (copy)/cart.ts",
            "O'Neill/file.txt",
            "R & D/a;b/with`backtick/cart.ts",
            "src/",
            "",
        ] {
            let unix = format!("/home/me/shop/{suffix}");
            let windows = format!("C:\\Users\\me\\shop\\{}", suffix.replace('/', "\\"));
            for (fix, input, expected) in [(&to_unix, &windows, &unix), (&to_windows, &unix, &windows)] {
                let json = serde_json::to_vec(&serde_json::json!({"file": input})).unwrap();
                let actual: serde_json::Value = serde_json::from_slice(&fix.fix(&json)).unwrap();
                assert_eq!(actual["file"], expected.as_str(), "{input}");
            }
        }
    }

    #[test]
    fn path_fixer_preserves_surrounding_text_and_json_escapes() {
        let fix = PathFixer::new(&[("/home/me/shop", r"C:\Users\me\shop")]).unwrap();
        let input = serde_json::json!({"text": "Edited /home/me/shop/my folder/file.txt and /other/file.txt\nThen /home/me/shop/src/; see https://example.com/a/b"});
        let actual: serde_json::Value = serde_json::from_slice(&fix.fix(&serde_json::to_vec(&input).unwrap())).unwrap();
        assert_eq!(
            actual["text"],
            "Edited C:\\Users\\me\\shop\\my folder\\file.txt and /other/file.txt\nThen C:\\Users\\me\\shop\\src\\; see https://example.com/a/b"
        );
        let back = PathFixer::new(&[(r"C:\Users\me\shop", "/home/me/shop")]).unwrap();
        let again: serde_json::Value =
            serde_json::from_slice(&back.fix(&serde_json::to_vec(&actual).unwrap())).unwrap();
        assert_eq!(again, input);
    }

    #[test]
    fn encode_and_resolve_claude_folder_names() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("Work").join("my-app_v2.0").join("src");
        std::fs::create_dir_all(&folder).unwrap();
        let folder = canonical(&folder).unwrap().to_string_lossy().into_owned();
        let name = encode_claude(&folder);
        assert!(!name.contains('/') && !name.contains('\\') && !name.contains('_') && !name.contains('.'));
        assert_eq!(resolve_encoded(&name), Some(folder));
    }
}
