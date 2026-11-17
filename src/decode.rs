//! Small pure decoders that turn option values into plain words: chmod modes, find tests,
//! signals, docker port and volume specs, simple sed/awk programs and git references.

fn list_and(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [one] => (*one).to_string(),
        [first @ .., last] => format!("{} and {last}", first.join(", ")),
    }
}

fn perm_words(bits: u32) -> Vec<&'static str> {
    let mut words = Vec::new();
    if bits & 4 != 0 {
        words.push("read");
    }
    if bits & 2 != 0 {
        words.push("write");
    }
    if bits & 1 != 0 {
        words.push("run");
    }
    words
}

/// Explains a chmod mode: `755`, `0644`, `u+x`, `go-w`, `a=rx`.
pub fn chmod_mode(mode: &str) -> Option<String> {
    if mode.is_empty() {
        return None;
    }
    if mode.chars().all(|c| ('0'..='7').contains(&c)) && (3..=4).contains(&mode.len()) {
        let digits: Vec<u32> = mode.chars().filter_map(|c| c.to_digit(8)).collect();
        let (special, rest) = if digits.len() == 4 { (digits[0], &digits[1..]) } else { (0, &digits[..]) };
        let (u, g, o) = (rest[0], rest[1], rest[2]);
        let describe = |bits: u32| {
            let w = perm_words(bits);
            if w.is_empty() { "nothing".to_string() } else { list_and(&w) }
        };
        let mut text = if u == g && g == o {
            if u == 0 {
                "nobody (not even the owner) can do anything with it".to_string()
            } else {
                format!("everyone (owner, group and others) can {}", describe(u))
            }
        } else {
            format!("the owner can {}; the group can {}; everyone else can {}", describe(u), describe(g), describe(o))
        };
        if special & 4 != 0 {
            text.push_str("; setuid: it runs with its owner's rights");
        }
        if special & 2 != 0 {
            text.push_str("; setgid");
        }
        if special & 1 != 0 {
            text.push_str("; sticky: only owners may delete inside a folder");
        }
        return Some(text);
    }
    let mut clauses = Vec::new();
    for clause in mode.split(',') {
        let op_at = clause.find(['+', '-', '='])?;
        let who = &clause[..op_at];
        let op = clause[op_at..].chars().next()?;
        let perms = &clause[op_at + 1..];
        if !who.chars().all(|c| "ugoa".contains(c)) || !perms.chars().all(|c| "rwxXstugo".contains(c)) {
            return None;
        }
        let who_text = if who.is_empty() || who.contains('a') {
            "everyone".to_string()
        } else {
            let mut parts = Vec::new();
            if who.contains('u') {
                parts.push("the owner");
            }
            if who.contains('g') {
                parts.push("the group");
            }
            if who.contains('o') {
                parts.push("others");
            }
            list_and(&parts)
        };
        let mut words = Vec::new();
        for c in perms.chars() {
            match c {
                'r' => words.push("read"),
                'w' => words.push("write"),
                'x' => words.push("run"),
                'X' => words.push("run (folders and already-runnable files)"),
                's' => words.push("setuid/setgid"),
                't' => words.push("sticky"),
                _ => words.push("the same permissions as another class"),
            }
        }
        let what = if words.is_empty() { "all permissions".to_string() } else { list_and(&words) };
        clauses.push(match op {
            '+' => format!("adds {what} permission for {who_text}"),
            '-' => format!("removes {what} permission from {who_text}"),
            _ => format!("sets {who_text} to exactly {what} permission"),
        });
    }
    if clauses.is_empty() { None } else { Some(clauses.join("; ")) }
}

/// Is this chmod mode one that opens a file to everyone for writing?
pub fn chmod_world_writable(mode: &str) -> bool {
    if mode.chars().all(|c| ('0'..='7').contains(&c)) && (3..=4).contains(&mode.len()) {
        return mode.chars().last().and_then(|c| c.to_digit(8)).is_some_and(|d| d & 2 != 0);
    }
    mode.split(',').any(|clause| {
        let Some(op_at) = clause.find(['+', '-', '=']) else { return false };
        let who = &clause[..op_at];
        clause[op_at..].starts_with(['+', '='])
            && (who.is_empty() || who.contains('a') || who.contains('o'))
            && clause[op_at + 1..].contains('w')
    })
}

fn split_sign(value: &str) -> Option<(char, u64, &str)> {
    let (sign, rest) = match value.chars().next()? {
        c @ ('+' | '-') => (c, &value[1..]),
        _ => ('=', value),
    };
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    let n = digits.parse().ok()?;
    Some((sign, n, &rest[digits.len()..]))
}

/// `-mtime +7` and friends. `flag` is the option name, `value` what follows it.
pub fn find_time(flag: &str, value: &str) -> Option<String> {
    let (what, unit) = match flag {
        "-mtime" => ("whose content was last changed", "day"),
        "-atime" => ("that were last read", "day"),
        "-ctime" => ("whose status (permissions, owner or content) last changed", "day"),
        "-mmin" => ("whose content was last changed", "minute"),
        "-amin" => ("that were last read", "minute"),
        "-cmin" => ("whose status last changed", "minute"),
        _ => return None,
    };
    let (sign, n, rest) = split_sign(value)?;
    if !rest.is_empty() {
        return None;
    }
    let plural = if n == 1 { "" } else { "s" };
    let when = match sign {
        '+' => format!("more than {n} {unit}{plural} ago"),
        '-' => format!("less than {n} {unit}{plural} ago (within the last {n} {unit}{plural})"),
        _ => format!("exactly {n} {unit}{plural} ago (counted in whole {unit}s)"),
    };
    Some(format!("match files {what} {when}"))
}

pub fn find_size(value: &str) -> Option<String> {
    let (sign, n, unit) = split_sign(value)?;
    let unit_text = match unit {
        "" | "b" => "512-byte blocks",
        "c" => "bytes",
        "w" => "2-byte words",
        "k" => "KiB",
        "M" => "MiB",
        "G" => "GiB",
        "T" => "TiB",
        _ => return None,
    };
    let cmp = match sign {
        '+' => "bigger than",
        '-' => "smaller than",
        _ => "exactly (rounded up to whole units)",
    };
    Some(format!("match files {cmp} {n} {unit_text}"))
}

pub fn find_type(value: &str) -> Option<String> {
    let mut kinds = Vec::new();
    for c in value.split(',') {
        kinds.push(match c {
            "f" => "regular files",
            "d" => "folders",
            "l" => "symbolic links",
            "b" => "block devices",
            "c" => "character devices",
            "p" => "named pipes",
            "s" => "sockets",
            _ => return None,
        });
    }
    Some(format!("match only {}", list_and(&kinds)))
}

pub struct Signal {
    pub number: i32,
    pub name: &'static str,
    pub meaning: &'static str,
}

const SIGNALS: [Signal; 12] = [
    Signal { number: 1, name: "HUP", meaning: "hang up: many programs reload their settings" },
    Signal { number: 2, name: "INT", meaning: "interrupt, the same as pressing Ctrl+C" },
    Signal { number: 3, name: "QUIT", meaning: "quit and dump the program's state" },
    Signal { number: 6, name: "ABRT", meaning: "abort" },
    Signal { number: 9, name: "KILL", meaning: "kill it immediately: the program cannot clean up or refuse" },
    Signal { number: 10, name: "USR1", meaning: "a user-defined signal (programs choose what it does)" },
    Signal { number: 12, name: "USR2", meaning: "a user-defined signal (programs choose what it does)" },
    Signal { number: 15, name: "TERM", meaning: "ask it to terminate politely (the default)" },
    Signal { number: 17, name: "CHLD", meaning: "a child process stopped or ended" },
    Signal { number: 18, name: "CONT", meaning: "continue a paused process" },
    Signal { number: 19, name: "STOP", meaning: "pause it (it cannot refuse)" },
    Signal { number: 20, name: "TSTP", meaning: "pause it, like Ctrl+Z" },
];

pub fn signal(text: &str) -> Option<&'static Signal> {
    let upper = text.to_ascii_uppercase();
    let name = upper.strip_prefix("SIG").unwrap_or(&upper);
    if let Ok(n) = name.parse::<i32>() {
        return SIGNALS.iter().find(|s| s.number == n);
    }
    SIGNALS.iter().find(|s| s.name == name)
}

pub fn is_kill_signal(text: &str) -> bool {
    signal(text).is_some_and(|s| s.number == 9)
}

pub fn port_mapping(spec: &str) -> Option<String> {
    let (ports, proto) = spec.split_once('/').unwrap_or((spec, "tcp"));
    let parts: Vec<&str> = ports.split(':').collect();
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit() || c == '-');
    match parts.as_slice() {
        [c] if digits(c) => Some(format!("container port {c}/{proto} on a random free host port")),
        [h, c] if digits(h) && digits(c) => {
            Some(format!("host port {h} is forwarded to port {c} inside the container (open localhost:{h}; {proto})"))
        }
        [ip, h, c] if digits(h) && digits(c) => {
            Some(format!("host port {h} on address {ip} is forwarded to port {c} inside the container ({proto})"))
        }
        _ => None,
    }
}

pub fn volume_spec(spec: &str) -> Option<String> {
    let parts: Vec<&str> = spec.split(':').collect();
    match parts.as_slice() {
        [src, dst] | [src, dst, _] if dst.starts_with('/') => {
            let opts = parts.get(2).copied().unwrap_or("");
            let mode = if opts.split(',').any(|o| o == "ro") { " (read-only)" } else { "" };
            let looks_like_path = src.starts_with(['/', '.', '~', '$']);
            if looks_like_path {
                Some(format!("{src} on this machine is shared as {dst} inside the container{mode}"))
            } else {
                Some(format!("the named docker volume {src} is mounted at {dst} inside the container{mode}"))
            }
        }
        [dst] if dst.starts_with('/') => {
            Some(format!("a fresh anonymous volume is mounted at {dst} inside the container"))
        }
        _ => None,
    }
}

/// `user:group`, `user`, `:group`.
pub fn owner_spec(spec: &str) -> Option<String> {
    if spec.is_empty() || spec.contains(' ') {
        return None;
    }
    let (user, group) = match spec.split_once([':', '.']) {
        Some((u, g)) => (u, Some(g)),
        None => (spec, None),
    };
    let mut parts = Vec::new();
    if !user.is_empty() {
        parts.push(format!("owner becomes {user}"));
    }
    match group {
        Some(g) if !g.is_empty() => parts.push(format!("group becomes {g}")),
        Some(_) => parts.push("group becomes the user's login group".to_string()),
        None => {}
    }
    if parts.is_empty() { None } else { Some(parts.join(", ")) }
}

fn split_unescaped(text: &str, delim: char) -> Vec<String> {
    let mut parts = vec![String::new()];
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            let cur = parts.last_mut().expect("non-empty");
            cur.push(c);
            if let Some(n) = chars.next() {
                cur.push(n);
            }
        } else if c == delim {
            parts.push(String::new());
        } else {
            parts.last_mut().expect("non-empty").push(c);
        }
    }
    parts
}

/// Explains simple sed scripts: `s/a/b/g`, `/x/d`, `5d`, `$d`, `2,4p`.
pub fn sed_script(script: &str) -> Option<String> {
    let s = script.trim();
    if let Some(rest) = s.strip_prefix('s') {
        let delim = rest.chars().next()?;
        if delim.is_alphanumeric() || delim.is_whitespace() || delim == '\\' {
            return None;
        }
        let parts = split_unescaped(&rest[delim.len_utf8()..], delim);
        if parts.len() < 3 {
            return None;
        }
        let (pat, repl, flags) = (&parts[0], &parts[1], &parts[2]);
        let mut text = if repl.is_empty() {
            format!("delete every match of `{pat}`")
        } else {
            format!("replace `{pat}` with `{repl}`")
        };
        let mut extra = Vec::new();
        if flags.contains('g') {
            extra.push("every occurrence on each line (not only the first)");
        } else {
            extra.push("only the first occurrence on each line");
        }
        if flags.contains('i') || flags.contains('I') {
            extra.push("ignoring upper/lower case");
        }
        if flags.contains('p') {
            extra.push("printing changed lines");
        }
        text.push_str(&format!(" ({})", extra.join(", ")));
        return Some(text);
    }
    if let Some(pat) = s.strip_suffix('d').and_then(|p| p.strip_prefix('/')).and_then(|p| p.strip_suffix('/')) {
        return Some(format!("delete the lines matching `{pat}`"));
    }
    if let Some(pat) = s.strip_suffix('p').and_then(|p| p.strip_prefix('/')).and_then(|p| p.strip_suffix('/')) {
        return Some(format!("print the lines matching `{pat}`"));
    }
    let (addr, cmd) = s.split_at(s.len().checked_sub(1)?);
    if !addr.is_empty()
        && addr.chars().all(|c| c.is_ascii_digit() || c == ',' || c == '$')
        && (cmd == "d" || cmd == "p")
    {
        let verb = if cmd == "d" { "delete" } else { "print" };
        return Some(match addr.split_once(',') {
            Some((a, b)) => format!("{verb} lines {a} to {b}"),
            None if addr == "$" => format!("{verb} the last line"),
            None => format!("{verb} line {addr}"),
        });
    }
    None
}

/// Explains simple awk programs like `{print $2}`.
pub fn awk_program(program: &str) -> Option<String> {
    let p = program.trim();
    let body = p.strip_prefix('{')?.strip_suffix('}')?.trim();
    let args = body.strip_prefix("print")?.trim();
    if args.is_empty() {
        return Some("print every line".to_string());
    }
    let mut fields = Vec::new();
    for part in args.split([',', ' ']).filter(|f| !f.is_empty()) {
        let name = part.strip_prefix('$')?;
        fields.push(match name {
            "0" => "the whole line".to_string(),
            "NF" => "the last column".to_string(),
            n if n.chars().all(|c| c.is_ascii_digit()) => format!("column {n}"),
            _ => return None,
        });
    }
    let refs: Vec<&str> = fields.iter().map(String::as_str).collect();
    Some(format!(
        "for every line, print {} (columns are separated by whitespace unless -F says otherwise)",
        list_and(&refs)
    ))
}

/// Notes on git references such as `HEAD~1` or `origin/main`.
pub fn git_ref(text: &str) -> Option<String> {
    if text == "HEAD" {
        return Some("HEAD is the commit you are on now".to_string());
    }
    if let Some(rest) = text.strip_prefix("HEAD~") {
        let n: u32 = if rest.is_empty() { 1 } else { rest.parse().ok()? };
        return Some(match n {
            1 => "HEAD~1 is the commit just before the current one".to_string(),
            n => format!("HEAD~{n} is {n} commits before the current one"),
        });
    }
    if text == "HEAD^" {
        return Some("HEAD^ is the commit just before the current one".to_string());
    }
    if let Some((remote, branch)) = text.split_once('/')
        && matches!(remote, "origin" | "upstream")
        && !branch.is_empty()
        && !branch.contains(' ')
        && !branch.starts_with('.')
    {
        return Some(format!("{text} is branch {branch} on the remote `{remote}`, as of the last fetch"));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chmod_modes() {
        assert_eq!(
            chmod_mode("755").unwrap(),
            "the owner can read, write and run; the group can read and run; everyone else can read and run"
        );
        assert!(chmod_mode("777").unwrap().starts_with("everyone (owner, group and others) can read, write and run"));
        assert!(
            chmod_mode("600").unwrap().contains("everyone else can nothing")
                || chmod_mode("600").unwrap().contains("nothing")
        );
        assert!(chmod_mode("4755").unwrap().contains("setuid"));
        assert_eq!(chmod_mode("+x").unwrap(), "adds run permission for everyone");
        assert_eq!(chmod_mode("go-w").unwrap(), "removes write permission from the group and others");
        assert_eq!(
            chmod_mode("u=rwx,g=rx").unwrap(),
            "sets the owner to exactly read, write and run permission; sets the group to exactly read and run permission"
        );
        assert!(chmod_mode("hello").is_none() && chmod_mode("999").is_none());
        assert!(chmod_world_writable("777") && chmod_world_writable("o+w") && chmod_world_writable("a+rwx"));
        assert!(!chmod_world_writable("755") && !chmod_world_writable("u+x"));
    }

    #[test]
    fn find_values() {
        assert_eq!(
            find_time("-mtime", "+7").unwrap(),
            "match files whose content was last changed more than 7 days ago"
        );
        assert!(find_time("-mtime", "-1").unwrap().contains("less than 1 day ago"));
        assert!(find_time("-mmin", "30").unwrap().contains("exactly 30 minutes ago"));
        assert!(find_time("-mtime", "abc").is_none());
        assert_eq!(find_size("+100M").unwrap(), "match files bigger than 100 MiB");
        assert_eq!(find_type("f").unwrap(), "match only regular files");
        assert_eq!(find_type("f,d").unwrap(), "match only regular files and folders");
    }

    #[test]
    fn signals() {
        assert_eq!(signal("9").unwrap().name, "KILL");
        assert_eq!(signal("sigterm").unwrap().number, 15);
        assert!(is_kill_signal("KILL") && !is_kill_signal("15"));
        assert!(signal("bogus").is_none());
    }

    #[test]
    fn docker_specs() {
        assert!(port_mapping("8080:80").unwrap().contains("host port 8080 is forwarded to port 80"));
        assert!(port_mapping("127.0.0.1:8080:80").unwrap().contains("127.0.0.1"));
        assert!(volume_spec("$PWD:/usr/share/nginx/html:ro").unwrap().contains("read-only"));
        assert!(volume_spec("data:/var/lib/db").unwrap().contains("named docker volume data"));
    }

    #[test]
    fn sed_and_awk() {
        assert_eq!(
            sed_script("s/foo/bar/g").unwrap(),
            "replace `foo` with `bar` (every occurrence on each line (not only the first))"
        );
        assert!(sed_script("s|a/b|c|").unwrap().contains("`a/b`"));
        assert_eq!(sed_script("/x/d").unwrap(), "delete the lines matching `x`");
        assert_eq!(sed_script("2,4p").unwrap(), "print lines 2 to 4");
        assert!(sed_script("y/abc/xyz/").is_none());
        assert!(awk_program("{print $2}").unwrap().contains("print column 2"));
        assert!(awk_program("{print $1, $NF}").unwrap().contains("column 1 and the last column"));
        assert!(awk_program("BEGIN{x=1}").is_none());
    }

    #[test]
    fn git_refs() {
        assert!(git_ref("HEAD~1").unwrap().contains("just before"));
        assert!(git_ref("HEAD~3").unwrap().contains("3 commits before"));
        assert!(git_ref("origin/main").unwrap().contains("branch main on the remote `origin`"));
        assert!(git_ref("main").is_none());
    }

    #[test]
    fn owners() {
        assert_eq!(owner_spec("www-data:www-data").unwrap(), "owner becomes www-data, group becomes www-data");
        assert_eq!(owner_spec(":staff").unwrap(), "group becomes staff");
    }
}
