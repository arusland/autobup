//! Per-project secrets file `~/.config/autobup/<project_name>.env`.
//!
//! One `NAME=value` per line; blank lines and `#` comments are ignored, an
//! optional `export ` prefix is allowed and matching surrounding quotes are
//! stripped. Config values can use the variables as `${NAME}`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// `~/.config/autobup/<project>.env`, or None without a home directory.
pub fn path(home: Option<&str>, project: &str) -> Option<PathBuf> {
    let home = home.filter(|h| !h.is_empty())?;
    Some(
        Path::new(home)
            .join(".config/autobup")
            .join(format!("{project}.env")),
    )
}

/// Read the secrets file; a missing file is empty.
pub fn load(file: &Path) -> Result<BTreeMap<String, String>, String> {
    match std::fs::read_to_string(file) {
        Ok(text) => parse(&text).map_err(|e| format!("secrets file {}: {e}", file.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(format!("cannot read secrets file {}: {e}", file.display())),
    }
}

pub fn parse(text: &str) -> Result<BTreeMap<String, String>, String> {
    let mut vars = BTreeMap::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("line {}: expected NAME=value", n + 1))?;
        let key = key.trim();
        if !is_name(key) {
            return Err(format!("line {}: invalid variable name {key:?}", n + 1));
        }
        vars.insert(key.to_string(), unquote(value.trim()).to_string());
    }
    Ok(vars)
}

fn unquote(v: &str) -> &str {
    for q in ['"', '\''] {
        if v.len() >= 2 && v.starts_with(q) && v.ends_with(q) {
            return &v[1..v.len() - 1];
        }
    }
    v
}

pub fn is_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Replace `${NAME}` in `s` using `lookup`. `${HOME}` is left alone (it is
/// expanded later, only in path settings). Every name found is added to
/// `used`; names `lookup` does not know are added to `missing` and replaced
/// with nothing.
pub fn substitute(
    s: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
    used: &mut BTreeSet<String>,
    missing: &mut BTreeSet<String>,
) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after
            .find('}')
            .filter(|&e| is_name(&after[..e]) && &after[..e] != "HOME")
        else {
            out.push_str("${");
            rest = after;
            continue;
        };
        let name = &after[..end];
        used.insert(name.to_string());
        match lookup(name) {
            Some(v) => out.push_str(&v),
            None => {
                missing.insert(name.to_string());
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// True if `s` refers to a variable instead of holding a literal secret.
pub fn has_reference(s: &str) -> bool {
    let mut used = BTreeSet::new();
    substitute(s, &|_| None, &mut used, &mut BTreeSet::new());
    !used.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_env_file() {
        let vars =
            parse("# comment\n\nPASSWORD=abc+/def=\nexport AWS_KEY = \"x y\"\nQ='a'\nEMPTY=\n").unwrap();
        assert_eq!(vars["PASSWORD"], "abc+/def=");
        assert_eq!(vars["AWS_KEY"], "x y");
        assert_eq!(vars["Q"], "a");
        assert_eq!(vars["EMPTY"], "");
        assert!(parse("no equals sign").unwrap_err().contains("line 1"));
        assert!(parse("1X=a").unwrap_err().contains("invalid variable name"));
    }

    #[test]
    fn substitute_vars() {
        let lookup = |k: &str| (k == "A").then(|| "1".to_string());
        let (mut used, mut missing) = (BTreeSet::new(), BTreeSet::new());
        let s = substitute("x${A}y${B}z${HOME}$A${}${a-b", &lookup, &mut used, &mut missing);
        assert_eq!(s, "x1yz${HOME}$A${}${a-b");
        assert_eq!(used, BTreeSet::from(["A".to_string(), "B".to_string()]));
        assert_eq!(missing, BTreeSet::from(["B".to_string()]));
        assert!(has_reference("${PASSWORD}"));
        assert!(!has_reference("plain ${HOME}"));
    }
}
