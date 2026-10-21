//! Paths: showing them, Claude Code's folder names, and rewriting another machine's paths.
//! Paths cross between Linux, macOS and Windows machines, so they're stored with `/` and
//! rewritten between the two flavors (`/home/me/shop` ⇄ `C:\Users\me\shop`).

use std::path::{Path, PathBuf};

use crate::ctx::ctx;

/// A path as this machine writes it: on Windows, `\` separators and no `\\?\` prefix.
pub fn native(path: &str) -> String {
    if !cfg!(windows) {
        return path.to_string();
    }
    let path = path.strip_prefix(r"\\?\").unwrap_or(path);
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        out.push(if c == '/' { '\\' } else { c });
    }
    out
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

fn name_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c >= 0x80 || b"_.-~@+=%#".contains(&c)
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
                    // /sub/dir after it: same names, this machine's separators
                    while let Some(sep) = rule.seps_in.iter().find(|s| data[end..].starts_with(s)) {
                        let name = data[end + sep.len()..].iter().take_while(|c| name_byte(**c)).count();
                        if name == 0 {
                            break;
                        }
                        let from = end + sep.len();
                        out.extend_from_slice(&rule.sep_out);
                        out.extend_from_slice(&data[from..from + name]);
                        end = from + name;
                    }
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
