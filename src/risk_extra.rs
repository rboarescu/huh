//! More risk checks for `huh explain`, written against the plain words of the command line so they
//! work for programs huh has no knowledge section for (ngrok, kubectl, aws, ...). Every warning
//! names one category from risk_kinds.rs. Nothing here runs anything.

use crate::risk::{Inv, Risk};

type Warn<'a> = &'a mut dyn FnMut(Risk, &str, String);

/// Splits a command line into words the way a shell does, minus the quotes.
fn tokens(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        match quote {
            Some(q) if c == q => quote = None,
            Some('"') if c == '\\' => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            Some(_) => cur.push(c),
            None => match c {
                '\'' | '"' => {
                    quote = Some(c);
                    started = true;
                }
                '\\' => {
                    if let Some(n) = chars.next() {
                        cur.push(n);
                        started = true;
                    }
                }
                c if c.is_whitespace() => {
                    if started || !cur.is_empty() {
                        out.push(std::mem::take(&mut cur));
                        started = false;
                    }
                }
                c => {
                    cur.push(c);
                    started = true;
                }
            },
        }
    }
    if started || !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(n, _)| {
        !n.is_empty()
            && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !n.starts_with(|c: char| c.is_ascii_digit())
    })
}

fn base(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

/// Does the value of an option such as `--host` listen on every network interface?
fn all_interfaces(addr: &str) -> bool {
    let a = addr.trim_matches(['[', ']']);
    let host = a.rsplit_once(':').map_or(a, |(h, port)| if port.chars().all(|c| c.is_ascii_digit()) { h } else { a });
    matches!(host.trim_matches(['[', ']']), "0.0.0.0" | "::" | "*" | "" | "0")
}

fn is_loopback(addr: &str) -> bool {
    let a = addr.trim_matches(['[', ']']);
    a.starts_with("127.") || a == "localhost" || a == "::1" || a.starts_with("localhost:") || a.starts_with("[::1]")
}

/// A file that holds credentials or a private key.
pub fn sensitive_file(path: &str) -> bool {
    let p = path.trim_start_matches('@').trim_start_matches('<');
    let p = p.split_once("=@").map_or(p, |(_, f)| f);
    let name = base(p.trim_end_matches('/'));
    let lower = name.to_ascii_lowercase();
    let private_key_name = lower.starts_with("id_") && !lower.ends_with(".pub");
    let key_ext = [".pem", ".key", ".p12", ".pfx", ".ppk", ".keystore", ".jks"].iter().any(|e| lower.ends_with(e));
    let env_file = lower == ".env"
        || (lower.starts_with(".env.")
            && !["example", "sample", "template", "dist"].iter().any(|s| lower.ends_with(s)));
    private_key_name
        || key_ext
        || env_file
        || [".netrc", ".git-credentials", ".npmrc", ".pypirc", "credentials.json"].contains(&lower.as_str())
        || p.ends_with(".aws/credentials")
        || p.ends_with(".docker/config.json")
        || p.ends_with(".config/gh/hosts.yml")
        || p.ends_with(".kube/config")
        || p.contains("/.gnupg/private-keys")
        || p.contains("/.gnupg/secring")
        || p == "/etc/shadow"
        || p == "/etc/gshadow"
}

/// A folder that holds credentials (`~/.ssh`, `~/.aws`, ...).
fn sensitive_dir(path: &str) -> bool {
    let p = path.trim_end_matches('/');
    [".ssh", ".aws", ".gnupg", ".kube", ".docker"].iter().any(|d| p.ends_with(&format!("/{d}")) || p == *d)
}

const READERS: [&str; 15] = [
    "cat", "tac", "less", "more", "head", "tail", "bat", "batcat", "xxd", "hexdump", "od", "strings", "base64", "nl",
    "view",
];
const SENDERS: [&str; 13] =
    ["scp", "rsync", "sftp", "curl", "wget", "nc", "ncat", "socat", "rclone", "ftp", "aws", "gsutil", "gh"];

const ROOT_EQUIVALENT_GROUPS: [&str; 12] =
    ["sudo", "wheel", "admin", "root", "docker", "lxd", "lxc", "disk", "shadow", "libvirt", "kvm", "adm"];

/// Environment variables whose value weakens something or hides something.
pub fn assignment_warning(name: &str, value: &str) -> Option<(Risk, String)> {
    if let Some(text) = crate::risk::secret_assignment(name, value) {
        return Some((Risk::LeaksSecrets, text));
    }
    let upper = name.to_ascii_uppercase();
    match upper.as_str() {
        "GIT_SSL_NO_VERIFY" if !matches!(value, "" | "0" | "false") => Some((
            Risk::WeakensSecurity,
            "makes git skip the check that the server's certificate is genuine: anyone on the network path can tamper with what you download or push.".into(),
        )),
        "NODE_TLS_REJECT_UNAUTHORIZED" | "PYTHONHTTPSVERIFY" | "DOCKER_TLS_VERIFY" | "NPM_CONFIG_STRICT_SSL"
            if matches!(value, "0" | "false") =>
        {
            Some((
                Risk::WeakensSecurity,
                format!("{name}={value} turns off certificate checking for the programs started with it: connections can be intercepted without any error."),
            ))
        }
        "GIT_SSH_COMMAND" if value.to_ascii_lowercase().replace(' ', "").contains("stricthostkeychecking=no") => Some((
            Risk::WeakensSecurity,
            "makes git's ssh skip the check of the server's identity: you cannot tell if someone is impersonating it.".into(),
        )),
        "HISTFILE" if value.is_empty() || value == "/dev/null" => Some((
            Risk::HidesTracks,
            "stops the shell from saving the commands you type: nothing from this session ends up in your history.".into(),
        )),
        "HISTSIZE" | "HISTFILESIZE" if value == "0" => Some((
            Risk::HidesTracks,
            format!("{name}=0 makes the shell keep no command history."),
        )),
        "LD_PRELOAD" if !value.is_empty() => Some((
            Risk::RunsUnreadCode,
            "loads the given library into every program started with it: its code runs inside them and can change what they do.".into(),
        )),
        "PATH" => {
            let dotted = value.split(':').enumerate().any(|(i, part)| {
                (part == "." || part.is_empty()) && (i == 0 || value.ends_with(':') || value.contains("::"))
            });
            (dotted && !value.is_empty()).then(|| {
                (
                    Risk::WeakensSecurity,
                    "puts the current folder (or an empty entry) in PATH: a program dropped into any folder you work in can then run instead of the real one.".to_string(),
                )
            })
        }
        _ => None,
    }
}

pub fn check(inv: &Inv, warn: Warn) {
    let all = tokens(&inv.raw);
    let start = all.iter().position(|w| !is_assignment(w)).unwrap_or(all.len());
    let Some(first) = all.get(start) else { return };
    let prog = base(first).to_string();
    let args: Vec<&str> = all[start + 1..].iter().map(String::as_str).collect();
    if args.is_empty() && !matches!(prog.as_str(), "ngrok" | "visudo" | "unset") {
        return;
    }
    let has = |names: &[&str]| args.iter().any(|a| names.contains(a));
    let word = |name: &str| args.iter().any(|a| a.eq_ignore_ascii_case(name));
    let after = |names: &[&str]| -> Option<&str> {
        for (i, a) in args.iter().enumerate() {
            for n in names {
                if a == n {
                    return args.get(i + 1).copied();
                }
                if let Some(v) = a.strip_prefix(&format!("{n}=")) {
                    return Some(v);
                }
            }
        }
        None
    };
    let positional: Vec<&str> = args.iter().copied().filter(|a| !a.starts_with('-')).collect();
    let sub = positional.first().copied().unwrap_or("");
    let joined = args.join(" ").to_ascii_lowercase();
    let compact = joined.replace([' ', '='], "");

    // --- certificate and host-key checks ---------------------------------------------------------
    if prog == "git" {
        let off = args.iter().any(|a| matches!(a.to_ascii_lowercase().as_str(), "false" | "0" | "no" | "off"))
            || joined.contains("sslverify=false")
            || joined.contains("sslverify=0");
        if joined.contains("sslverify") && off {
            warn(
                Risk::WeakensSecurity,
                "http.sslVerify",
                "turns off git's check that the server's certificate is genuine: anyone on the network path can tamper with what you download or push. Fix the certificate (or point http.sslCAInfo at the right one) instead.".into(),
            );
        }
        if has(&["--no-gpg-sign"]) || compact.contains("gpgsignfalse") {
            warn(
                Risk::WeakensSecurity,
                "gpgsign",
                "leaves the commit unsigned although signing is set up: it no longer proves who made it.".into(),
            );
        }
        if matches!(sub, "commit" | "push" | "merge" | "am")
            && (has(&["--no-verify"]) || (sub == "commit" && has(&["-n"])))
        {
            warn(
                Risk::WeakensSecurity,
                if has(&["--no-verify"]) { "--no-verify" } else { "-n" },
                format!(
                    "skips the {} hooks, the checks (tests, lint, secret scans) the project set up to run first.",
                    match sub {
                        "commit" => "pre-commit and commit-msg",
                        "push" => "pre-push",
                        _ => "merge",
                    }
                ),
            );
        }
        if sub == "config" && joined.contains("credential.helper") && positional.contains(&"store") {
            warn(
                Risk::LeaksSecrets,
                "credential.helper",
                "saves your git passwords and tokens as plain text in ~/.git-credentials, readable by any program you run.".into(),
            );
        }
        if sub == "config"
            && joined.contains("safe.directory")
            && !has(&["--unset", "--unset-all", "--get", "--list", "-l"])
        {
            let everything = positional.contains(&"*");
            warn(
                Risk::WeakensSecurity,
                "safe.directory",
                format!(
                    "tells git to trust {} although another user owns it: hooks and settings inside it will then run as you, so only do this for a folder you trust.",
                    if everything { "every folder" } else { "that folder" }
                ),
            );
        }
        if sub == "remote" && positional.get(1).is_some_and(|p| *p == "set-url") {
            warn(
                Risk::AffectsOthers,
                "set-url",
                "changes where `git push` and `git pull` go: later pushes send your code, and anything in it, to the new address.".into(),
            );
        }
        if sub == "update-ref" && has(&["-d"]) {
            warn(
                Risk::RewritesHistory,
                "update-ref",
                "deletes a branch or other reference directly, without the checks `git branch -d` does.".into(),
            );
        }
        if sub == "push" && !has(&["-n", "--dry-run"]) {
            const SHARED: [&str; 7] = ["main", "master", "trunk", "develop", "production", "prod", "stable"];
            let target = positional.iter().skip(1).find(|p| {
                let b = p.rsplit(':').next().unwrap_or(p).trim_start_matches('+').trim_start_matches("refs/heads/");
                SHARED.contains(&b) || b.starts_with("release")
            });
            if let Some(branch) = target {
                warn(
                    Risk::AffectsOthers,
                    branch,
                    format!(
                        "sends your commits to `{branch}`, a branch other people and automatic deploys usually rely on; make sure that is where this belongs (a branch plus a pull request is the usual route)."
                    ),
                );
            }
        }
    }
    if matches!(prog.as_str(), "scp" | "sftp" | "rsync" | "ssh-copy-id" | "sshfs" | "ssh")
        && compact.to_ascii_lowercase().contains("userknownhostsfile/dev/null")
    {
        warn(
            Risk::WeakensSecurity,
            "UserKnownHostsFile=/dev/null",
            "forgets every server's identity: ssh cannot warn you any more when a server is replaced by an impostor."
                .into(),
        );
    }
    if matches!(prog.as_str(), "scp" | "sftp" | "rsync" | "ssh-copy-id" | "sshfs")
        && (compact.to_ascii_lowercase().contains("stricthostkeycheckingno")
            || compact.to_ascii_lowercase().contains("stricthostkeycheckingoff"))
    {
        warn(
            Risk::WeakensSecurity,
            "StrictHostKeyChecking=no",
            "skips the check of the server's identity: you cannot tell if someone is impersonating it.".into(),
        );
    }
    if matches!(prog.as_str(), "npm" | "pnpm" | "yarn") && compact.contains("strict-sslfalse") {
        warn(
            Risk::WeakensSecurity,
            "strict-ssl",
            "turns off certificate checking for the npm registry: packages can be swapped on the way.".into(),
        );
    }
    if matches!(prog.as_str(), "pip" | "pip3" | "python" | "python3" | "uv" | "pipx") {
        if has(&["--trusted-host"]) || joined.contains("--trusted-host=") {
            warn(Risk::WeakensSecurity, "--trusted-host", "tells pip to trust that server without a valid certificate: packages from it can be swapped on the way.".into());
        }
        if let Some(url) = after(&["-i", "--index-url"])
            && url.starts_with("http://")
        {
            warn(
                Risk::WeakensSecurity,
                "--index-url",
                "downloads packages over plain http: anyone on the network path can replace them.".into(),
            );
        }
        if has(&["--extra-index-url"]) || joined.contains("--extra-index-url=") {
            warn(Risk::RunsInternetCode, "--extra-index-url", "also takes packages from another server: if a package of the same name exists there, it can win over the real one.".into());
        }
    }
    if matches!(prog.as_str(), "pip" | "pip3" | "uv" | "npm" | "pnpm" | "yarn" | "cargo")
        && args
            .iter()
            .any(|a| a.starts_with("git+") || a.starts_with("http://") || a.starts_with("https://") || *a == "--git")
        && !matches!(sub, "config" | "view" | "search")
    {
        warn(
            Risk::RunsInternetCode,
            "git+...",
            "installs straight from a web or git address: whatever is there now is downloaded and its build or install code runs on this machine.".into(),
        );
    }
    if matches!(
        prog.as_str(),
        "apt" | "apt-get" | "dnf" | "yum" | "zypper" | "pacman" | "yay" | "paru" | "rpm" | "dpkg"
    ) {
        for flag in [
            "--allow-unauthenticated",
            "--nosignature",
            "--nodigest",
            "--nogpgcheck",
            "--no-gpg-checks",
            "--skip-verify",
            "--force-yes",
            "--allow-insecure-repositories",
        ] {
            if has(&[flag]) {
                warn(Risk::WeakensSecurity, flag, "installs without checking the package signature: a tampered or fake package goes straight onto the system.".into());
            }
        }
        if matches!(prog.as_str(), "pacman" | "yay" | "paru")
            && (has(&["--overwrite"]) || joined.contains("--overwrite="))
        {
            warn(
                Risk::DestroysData,
                "--overwrite",
                "replaces files that belong to other packages, which can break them.".into(),
            );
        }
        if has(&["-y", "--yes", "--assume-yes", "--noconfirm"]) && !matches!(prog.as_str(), "rpm" | "dpkg") {
            warn(
                Risk::BigSystemChange,
                if has(&["--noconfirm"]) { "--noconfirm" } else { "-y" },
                "answers yes to every question, so removals and replacements go through without asking.".into(),
            );
        }
    }
    if matches!(prog.as_str(), "snap" | "flatpak")
        && matches!(sub, "install" | "remove" | "uninstall" | "refresh" | "update" | "revert")
    {
        warn(Risk::BigSystemChange, sub, "changes the software installed on the whole system.".into());
    }
    if prog == "deno" && sub == "run" {
        if has(&["-A", "--allow-all"]) {
            warn(
                Risk::WeakensSecurity,
                "--allow-all",
                "gives the script every permission (files, network, running programs) deno would otherwise ask about."
                    .into(),
            );
        }
        if positional.iter().any(|p| p.starts_with("http://") || p.starts_with("https://")) {
            warn(Risk::RunsInternetCode, "deno run", "runs a script straight from a web address.".into());
        }
    }
    if matches!(prog.as_str(), "npx" | "bunx" | "uvx" | "pipx") && (prog != "pipx" || sub == "run")
        || (matches!(prog.as_str(), "pnpm" | "yarn") && sub == "dlx")
        || (prog == "npm" && sub == "exec")
    {
        warn(
            Risk::RunsInternetCode,
            if prog == "pipx" || prog == "npm" || prog == "pnpm" || prog == "yarn" { sub } else { prog.as_str() },
            "downloads a package and runs it right away (unless it is already installed): its code is not looked at first.".into(),
        );
    }

    // --- ssh and tunnels ---------------------------------------------------------------------------
    if prog == "ssh" {
        if has(&["-A"]) || compact.to_ascii_lowercase().contains("forwardagentyes") {
            warn(
                Risk::GivesAwayAccess,
                if has(&["-A"]) { "-A" } else { "ForwardAgent=yes" },
                "lets the remote computer use your ssh keys while you are logged in: anyone with admin rights there can log in to other servers as you.".into(),
            );
        }
        if let Some(spec) = after(&["-R"]) {
            let wide = spec.starts_with("0.0.0.0:") || spec.starts_with("*:") || spec.starts_with(':');
            warn(
                Risk::ExposesService,
                "-R",
                format!(
                    "opens port {} on the remote computer and sends what arrives there to a port on this side: anyone who can reach that remote port can reach your service{}.",
                    spec.split(':').next().filter(|s| !s.is_empty()).unwrap_or("(chosen)"),
                    if wide {
                        ", from the whole network"
                    } else {
                        " (the server's GatewayPorts setting decides who that is)"
                    }
                ),
            );
        }
        for flag in ["-L", "-D"] {
            if let Some(spec) = after(&[flag])
                && (spec.starts_with("0.0.0.0:") || spec.starts_with("*:") || spec.starts_with(':') && flag == "-L")
            {
                warn(Risk::ExposesService, flag, "listens on every network interface of this computer, so other machines can use the tunnel, not just you.".into());
            }
        }
        if has(&["-g"]) {
            warn(Risk::ExposesService, "-g", "lets other computers use the forwarded ports, not just this one.".into());
        }
    }
    if prog == "ssh-keygen"
        && (joined.contains("-n \"\"")
            || joined.contains("-n ''")
            || args.windows(2).any(|w| w[0] == "-N" && w[1].is_empty()))
    {
        warn(
            Risk::WeakensSecurity,
            "-N",
            "creates the key with no passphrase: anyone who gets the key file can use it.".into(),
        );
    }
    if prog == "ssh-copy-id" {
        warn(Risk::GivesAwayAccess, "ssh-copy-id", "adds your public key to the other computer's authorized keys: that key can then log in there without a password.".into());
    }
    if prog == "sshpass"
        && let Some(pw) = after(&["-p"])
        && !pw.is_empty()
    {
        warn(Risk::LeaksSecrets, "-p", "puts a password right on the command line: it is saved in your shell history and any user on the machine can read it in the process list while this runs. Use `sshpass -e` or `-f file` instead.".into());
    }

    // --- services that become reachable ------------------------------------------------------------
    let python = matches!(prog.as_str(), "python" | "python3" | "python2") || prog.starts_with("python3.");
    if python && (args.windows(2).any(|w| w[0] == "-m" && (w[1] == "http.server" || w[1] == "SimpleHTTPServer"))) {
        let bind = after(&["--bind", "-b"]);
        if !bind.is_some_and(is_loopback) {
            warn(
                Risk::ExposesService,
                "http.server",
                "serves the folder to every computer that can reach this one, not just you, and lists every file in it. Add `--bind 127.0.0.1` to keep it local.".into(),
            );
        }
    }
    if prog == "php"
        && let Some(addr) = after(&["-S"])
        && !is_loopback(addr)
    {
        warn(
            Risk::ExposesService,
            "-S",
            "starts a web server that listens on every network interface unless the address is localhost.".into(),
        );
    }
    if matches!(prog.as_str(), "http-server" | "live-server" | "serve" | "browser-sync")
        || (matches!(prog.as_str(), "npx" | "bunx" | "pnpm" | "yarn")
            && positional.iter().any(|p| matches!(*p, "http-server" | "live-server" | "serve" | "browser-sync")))
    {
        warn(Risk::ExposesService, "http-server", "serves a folder on every network interface by default, so other computers can browse it. Bind it to 127.0.0.1 (--address / --host) to keep it local.".into());
    }
    if !matches!(prog.as_str(), "ssh" | "git" | "docker" | "sshd" | "kubectl") {
        let mut host_flag = None;
        for (i, a) in args.iter().enumerate() {
            let name = a.split('=').next().unwrap_or(a);
            if matches!(
                name,
                "--host" | "--hostname" | "--bind" | "--listen" | "--address" | "-H" | "--addr" | "--bind-address"
            ) {
                let value = a
                    .split_once('=')
                    .map(|(_, v)| v)
                    .or_else(|| args.get(i + 1).copied().filter(|n| !n.starts_with('-')));
                match value {
                    Some(v) if all_interfaces(v) => host_flag = Some((*a).to_string()),
                    None if name == "--host"
                        && matches!(
                            prog.as_str(),
                            "vite" | "npm" | "pnpm" | "yarn" | "npx" | "astro" | "next" | "nuxt" | "ng"
                        ) =>
                    {
                        host_flag = Some((*a).to_string())
                    }
                    _ => {}
                }
            }
        }
        if let Some(flag) = host_flag {
            warn(
                Risk::ExposesService,
                &flag,
                "listens on every network interface, so other computers can reach it, not just this one. Use 127.0.0.1 to keep it local.".into(),
            );
        }
    }
    if matches!(prog.as_str(), "nc" | "ncat" | "netcat" | "socat") {
        let listening = if prog == "socat" {
            args.iter()
                .any(|a| a.to_ascii_uppercase().contains("-LISTEN:") || a.to_ascii_uppercase().starts_with("TCP-L"))
                && !joined.contains("bind=")
        } else {
            args.iter().any(|a| a.starts_with('-') && !a.starts_with("--") && a[1..].contains('l'))
                || has(&["--listen"])
        };
        if listening && !has(&["-s"]) {
            warn(
                Risk::ExposesService,
                "listen",
                "waits for connections on every network interface, so other computers can connect to it.".into(),
            );
        }
        let shell = (prog != "socat" && (has(&["-e", "-c", "--exec", "--sh-exec"])))
            || args.iter().any(|a| {
                let u = a.to_ascii_uppercase();
                u.starts_with("EXEC:") || u.starts_with("SYSTEM:")
            });
        if shell {
            warn(Risk::GivesAwayAccess, "-e", "connects a shell or program to the network: whoever is on the other end can run commands on this machine.".into());
        }
    }
    if prog == "ngrok"
        || (prog == "cloudflared"
            && matches!(sub, "tunnel" | "access")
            && !positional.iter().any(|p| matches!(*p, "list" | "login" | "info" | "delete")))
        || matches!(prog.as_str(), "lt" | "localtunnel" | "bore" | "serveo" | "frpc")
        || (prog == "tailscale" && (sub == "funnel" && !word("status") && !word("reset") || has(&["--funnel"])))
    {
        warn(
            Risk::ExposesService,
            prog.as_str(),
            "publishes a port of this computer on a public internet address: anyone who has the link can reach whatever listens there. Close it when you are done.".into(),
        );
    }
    if prog == "kubectl" {
        if sub == "port-forward"
            && let Some(addr) = after(&["--address"])
            && addr.split(',').any(all_interfaces)
        {
            warn(Risk::ExposesService, "--address", "opens the forwarded port on every network interface, so other computers can reach the cluster service.".into());
        }
        if sub == "proxy"
            && let Some(addr) = after(&["--address"])
            && all_interfaces(addr)
        {
            warn(
                Risk::ExposesService,
                "--address",
                "opens the kubernetes API proxy to the whole network, with your credentials behind it.".into(),
            );
        }
        if matches!(
            sub,
            "cordon"
                | "uncordon"
                | "taint"
                | "patch"
                | "edit"
                | "label"
                | "annotate"
                | "set"
                | "expose"
                | "run"
                | "autoscale"
                | "cp"
        ) {
            warn(
                Risk::AffectsOthers,
                sub,
                "changes the live cluster (the current kubectl context decides which one).".into(),
            );
        }
        if sub == "config" && positional.get(1).is_some_and(|p| matches!(*p, "use-context" | "set-context")) {
            warn(Risk::AffectsOthers, "use-context", "switches which cluster every later kubectl command acts on; check the name before you apply or delete anything.".into());
        }
        if let Some(ctx) = after(&["--context", "-n", "--namespace"])
            && ["prod", "production", "live"].iter().any(|p| ctx.to_ascii_lowercase().contains(p))
        {
            warn(Risk::AffectsOthers, ctx, "names what looks like production: changes there reach real users.".into());
        }
        if matches!(sub, "get" | "describe")
            && positional.iter().skip(1).any(|p| p.starts_with("secret"))
            && (joined.contains("-o yaml")
                || joined.contains("-o json")
                || joined.contains("-oyaml")
                || joined.contains("-ojson")
                || joined.contains("jsonpath")
                || joined.contains("go-template")
                || joined.contains("--output"))
        {
            warn(Risk::LeaksSecrets, "secrets", "prints secret values (base64 is only an encoding, not protection) to the screen, your scrollback and any log of this terminal.".into());
        }
        if sub == "logs" && has(&["-f", "--follow"]) {
            warn(Risk::HeadsUp, "-f", "keeps running and printing new lines until you press Ctrl+C.".into());
        }
    }

    // --- docker ------------------------------------------------------------------------------------
    if prog == "docker" || prog == "podman" {
        let is_run = positional.iter().take(2).any(|p| matches!(*p, "run" | "create"));
        if is_run {
            if after(&["--net", "--network"]).is_some_and(|v| v == "host") {
                warn(Risk::ExposesService, "--network host", "shares the host's network: every port the program opens is open on this computer's own addresses, with no mapping in between.".into());
            }
            for flag in ["--pid", "--ipc", "--userns", "--uts"] {
                if after(&[flag]) == Some("host") {
                    warn(Risk::GivesAwayAccess, flag, "shares that part of the host with the container, so it can see or affect the host's processes and memory.".into());
                }
            }
            if let Some(cap) = after(&["--cap-add"])
                && matches!(
                    cap.to_ascii_uppercase().as_str(),
                    "ALL" | "SYS_ADMIN" | "SYS_PTRACE" | "SYS_MODULE" | "NET_ADMIN" | "DAC_READ_SEARCH"
                )
            {
                warn(
                    Risk::GivesAwayAccess,
                    "--cap-add",
                    format!("gives the container the {cap} capability, which is close to root over the host."),
                );
            }
            if let Some(opt) = after(&["--security-opt"])
                && opt.contains("unconfined")
            {
                warn(
                    Risk::WeakensSecurity,
                    "--security-opt",
                    "turns off the container's seccomp or AppArmor protection.".into(),
                );
            }
            if has(&["-P", "--publish-all"]) {
                warn(
                    Risk::ExposesService,
                    "-P",
                    "publishes every exposed port on every network interface, so other computers can reach them."
                        .into(),
                );
            }
        }
        if sub == "push" || (sub == "image" && positional.get(1).is_some_and(|p| *p == "push")) {
            warn(Risk::AffectsOthers, "push", "uploads the image to a registry; if the repository is public or shared, everyone can pull it, and anything baked into it (keys, .env files) goes along.".into());
        }
        if matches!(sub, "volume" | "network" | "image" | "container")
            && positional.get(1).is_some_and(|p| matches!(*p, "rm" | "prune" | "remove"))
        {
            warn(
                Risk::DestroysData,
                positional[1],
                format!(
                    "deletes the {sub}s named (or all unused ones){}.",
                    if sub == "volume" { " and the data in them" } else { "" }
                ),
            );
        }
        if sub == "login" && has(&["-p", "--password"]) {
            warn(Risk::LeaksSecrets, "-p", "puts the registry password right on the command line: it is saved in your shell history and visible in the process list. Use --password-stdin.".into());
        }
    }

    // --- secrets -----------------------------------------------------------------------------------
    let sensitive: Vec<&str> = args.iter().copied().filter(|a| sensitive_file(a)).collect();
    if let Some(file) = sensitive.first() {
        if READERS.contains(&prog.as_str()) {
            warn(
                Risk::LeaksSecrets,
                file,
                format!(
                    "prints {file} (a private key, password file or stored credentials) to the screen, where your scrollback, screen sharing or a log of this terminal keeps a copy."
                ),
            );
        } else if SENDERS.contains(&prog.as_str()) {
            warn(
                Risk::LeaksSecrets,
                file,
                format!(
                    "sends {file} (a private key, password file or stored credentials) to another computer: whoever receives it can act as you."
                ),
            );
        }
    }
    if matches!(prog.as_str(), "tar" | "zip" | "7z" | "rar") && args.iter().any(|a| sensitive_dir(a)) {
        warn(
            Risk::LeaksSecrets,
            "archive",
            "packs a folder of keys and credentials into one file, which is then easy to copy or upload by mistake."
                .into(),
        );
    }
    if matches!(prog.as_str(), "echo" | "printf")
        && inv.pos.iter().any(|p| p.vars.iter().any(|(n, _)| secret_like_name(n)))
    {
        warn(
            Risk::LeaksSecrets,
            "echo",
            "prints the value of a secret variable to the screen, your scrollback and any log of this terminal.".into(),
        );
    }
    if prog == "gh" && sub == "auth" && (positional.get(1) == Some(&"token") || has(&["-t", "--show-token"])) {
        warn(Risk::LeaksSecrets, "auth", "prints your GitHub access token to the screen.".into());
    }
    if prog == "gpg" && has(&["--export-secret-keys", "--export-secret-subkeys"]) {
        warn(
            Risk::LeaksSecrets,
            "--export-secret-keys",
            "outputs your private key: anyone who gets it can sign and decrypt as you.".into(),
        );
    }
    if prog == "gpg" && has(&["--delete-secret-keys", "--delete-secret-and-public-key"]) {
        warn(
            Risk::DestroysData,
            "--delete-secret-keys",
            "deletes your private key; without a backup, what it protects can never be decrypted again.".into(),
        );
    }
    if prog == "aws" && positional.first() == Some(&"configure") && positional.iter().any(|p| p.contains("secret")) {
        warn(Risk::LeaksSecrets, "configure", "prints the stored AWS secret key.".into());
    }
    for a in args.iter().filter(|_| prog != "export") {
        if known_token(a) {
            warn(Risk::LeaksSecrets, &a.chars().take(12).collect::<String>(), "looks like an access token written right in the command: it is saved in your shell history and visible in the process list. Put it in a file or environment variable instead.".into());
            break;
        }
    }

    // --- erasing traces ------------------------------------------------------------------------------
    if prog == "history" && has(&["-c", "-d"]) {
        warn(Risk::HidesTracks, if has(&["-c"]) { "-c" } else { "-d" }, "erases the shell's record of the commands you ran; nothing shows afterwards what was done in this session.".into());
    }
    if prog == "unset" && args.iter().any(|a| matches!(*a, "HISTFILE" | "HISTSIZE" | "HISTFILESIZE")) {
        warn(
            Risk::HidesTracks,
            "HISTFILE",
            "stops the shell from saving the commands of this session to your history.".into(),
        );
    }
    if prog == "set" && joined.contains("+o history") {
        warn(Risk::HidesTracks, "+o history", "turns the shell's command history off for this session.".into());
    }
    if matches!(prog.as_str(), "rm" | "shred" | "unlink" | "truncate" | "srm" | "wipe")
        || (prog == "cp" && args.contains(&"/dev/null"))
    {
        for a in &args {
            if let Some(text) = crate::risk::trace_wipe_text(a) {
                warn(Risk::HidesTracks, a, text);
                break;
            }
        }
    }
    if prog == "find" && args.iter().any(|a| a.starts_with("/var/log")) && has(&["-delete"]) {
        warn(Risk::HidesTracks, "/var/log", "deletes log files, the record of what happened on this machine.".into());
    }
    if prog == "journalctl" && args.iter().any(|a| a.starts_with("--vacuum")) {
        warn(
            Risk::HidesTracks,
            "--vacuum",
            "deletes old journal entries for good; the system log no longer shows what happened back then.".into(),
        );
    }
    if prog == "dmesg" && has(&["-C", "--clear", "-c", "--read-clear"]) {
        warn(
            Risk::HidesTracks,
            "--clear",
            "empties the kernel message buffer, which holds hardware and security messages from boot.".into(),
        );
    }
    if prog == "auditctl" && (compact == "-e0" || has(&["-D"])) {
        warn(
            Risk::HidesTracks,
            "auditctl",
            "switches off auditing or deletes the audit rules: actions on this machine stop being recorded.".into(),
        );
    }
    if prog == "touch"
        && (has(&["-d", "--date", "-t", "-r", "--reference"])
            || args.iter().any(|a| a.starts_with("--date=") || a.starts_with("--reference=")))
    {
        warn(
            Risk::HidesTracks,
            "touch",
            "sets a file's timestamp to something other than now: it no longer shows when it was really changed."
                .into(),
        );
    }
    if prog == "systemctl" && matches!(sub, "stop" | "disable" | "mask" | "kill") {
        let units: Vec<&&str> = positional.iter().skip(1).collect();
        let named = |names: &[&str]| {
            units.iter().any(|u| names.iter().any(|n| u.trim_end_matches(".service") == *n || u.starts_with(n)))
        };
        if named(&["auditd", "rsyslog", "syslog", "syslog-ng", "systemd-journald"]) {
            warn(
                Risk::HidesTracks,
                "log service",
                "stops the service that records what happens on this machine; events while it is off are not logged."
                    .into(),
            );
        }
        if named(&[
            "firewalld",
            "ufw",
            "nftables",
            "iptables",
            "apparmor",
            "selinux",
            "fail2ban",
            "crowdsec",
            "clamav-daemon",
            "sshguard",
        ]) {
            warn(Risk::WeakensSecurity, "security service", "turns off a security service (firewall, access control or intrusion blocking): the protection it gave is gone until it is started again.".into());
        }
    }

    // --- security switches ---------------------------------------------------------------------------
    match prog.as_str() {
        "setenforce" if positional.first().is_some_and(|p| *p == "0" || p.eq_ignore_ascii_case("permissive")) => {
            warn(Risk::WeakensSecurity, "setenforce 0", "puts SELinux in permissive mode: it only logs violations instead of blocking them, until the next reboot.".into());
        }
        "aa-teardown" | "aa-disable" | "aa-complain" => {
            warn(
                Risk::WeakensSecurity,
                prog.as_str(),
                "unloads or relaxes AppArmor profiles: the programs they confined run unrestricted.".into(),
            );
        }
        "mokutil" if has(&["--disable-validation"]) => {
            warn(
                Risk::WeakensSecurity,
                "--disable-validation",
                "turns off Secure Boot signature checks for kernel modules and the boot chain.".into(),
            );
        }
        "ufw" if sub == "default" && positional.get(1) == Some(&"allow") => {
            warn(
                Risk::WeakensSecurity,
                "default allow",
                "lets traffic through by default: everything no rule blocks is open to the network.".into(),
            );
        }
        "firewall-cmd"
            if args.iter().any(|a| {
                a.starts_with("--add-port") || a.starts_with("--add-service") || a.starts_with("--add-rich-rule")
            }) =>
        {
            warn(
                Risk::ExposesService,
                "--add-port",
                "opens a port or service in the firewall, so other computers can reach it.".into(),
            );
        }
        "firewall-cmd"
            if args.iter().any(|a| a.starts_with("--set-default-zone") && a.contains("trusted"))
                || has(&["--panic-off"]) =>
        {
            warn(Risk::WeakensSecurity, "firewall-cmd", "relaxes the firewall for the whole machine.".into());
        }
        "cryptsetup"
            if matches!(sub, "luksFormat" | "luksErase" | "luksRemoveKey" | "erase" | "luksKillSlot" | "reencrypt") =>
        {
            warn(Risk::DestroysData, sub, "destroys the encryption header or key slot: without a header backup, what is on the disk may be lost for good.".into());
        }
        "hdparm"
            if args.iter().any(|a| {
                a.starts_with("--security-erase")
                    || a.starts_with("--trim-sector-ranges")
                    || *a == "--yes-i-know-what-i-am-doing"
            }) =>
        {
            warn(
                Risk::DestroysData,
                "--security-erase",
                "wipes the whole drive at firmware level; nothing on it can be recovered.".into(),
            );
        }
        "nvme" if matches!(sub, "format" | "sanitize") => {
            warn(Risk::DestroysData, sub, "erases the whole drive; nothing on it can be recovered.".into());
        }
        "chattr" if args.iter().any(|a| a.starts_with('-') && a.contains('i')) || joined.contains("-i ") => {
            warn(
                Risk::WeakensSecurity,
                "-i",
                "removes the immutable flag, the protection that stops even root from changing the file.".into(),
            );
        }
        "sysctl" if has(&["-w", "--write"]) || positional.iter().any(|p| p.contains('=')) => {
            let weakening = positional.iter().any(|p| {
                let l = p.to_ascii_lowercase();
                [
                    "randomize_va_space=0",
                    "ptrace_scope=0",
                    "kptr_restrict=0",
                    "rp_filter=0",
                    "unprivileged_userns_clone=1",
                    "unprivileged_bpf_disabled=0",
                    "dmesg_restrict=0",
                    "protected_symlinks=0",
                    "protected_hardlinks=0",
                ]
                .iter()
                .any(|k| l.ends_with(k))
            });
            if weakening {
                warn(Risk::WeakensSecurity, "sysctl", "turns off a kernel hardening setting.".into());
            }
            warn(Risk::BigSystemChange, "sysctl", "changes a running kernel setting for the whole machine (until reboot, unless it is also saved in /etc/sysctl.d).".into());
        }
        "sysctl" if has(&["-p", "--system", "--load"]) => {
            warn(Risk::BigSystemChange, "sysctl", "loads kernel settings from files into the running system.".into());
        }
        "insmod" | "rmmod" => {
            warn(
                Risk::BigSystemChange,
                prog.as_str(),
                "loads or removes kernel code; a wrong module can crash the machine or hide things from it.".into(),
            );
        }
        "modprobe" => {
            warn(
                Risk::BigSystemChange,
                if has(&["-r", "--remove"]) { "-r" } else { "modprobe" },
                if has(&["-r", "--remove"]) {
                    "removes a kernel module: hardware or features that use it stop working until it is loaded again."
                        .into()
                } else {
                    "loads a kernel module, which is code that runs inside the kernel with full rights.".into()
                },
            );
        }
        "update-grub" | "grub-install" | "grub-mkconfig" | "efibootmgr" | "bootctl" | "mkinitcpio" | "dracut"
        | "lilo" | "refind-install" => {
            if !(prog == "efibootmgr"
                && !has(&["-B", "-b", "-o", "-c", "-n", "-N", "-O", "--delete-bootnum", "--create"]))
                && !(prog == "bootctl" && matches!(sub, "status" | "list"))
            {
                warn(
                    Risk::BigSystemChange,
                    prog.as_str(),
                    "changes how the machine boots: a mistake can leave it unable to start.".into(),
                );
            }
        }
        "ip" if matches!(sub, "link" | "route" | "addr" | "address")
            && (word("down") || word("flush") || word("del") || word("delete") || word("replace")) =>
        {
            warn(
                Risk::BigSystemChange,
                sub,
                "changes the network setup of the machine: over ssh this can cut your own connection.".into(),
            );
        }
        "resolvectl" | "networkctl" | "nmcli"
            if matches!(sub, "dns" | "domain" | "down" | "radio" | "device" | "connection" | "reload")
                && args.iter().any(|a| matches!(*a, "dns" | "down" | "off" | "modify" | "delete" | "disconnect")) =>
        {
            warn(
                Risk::BigSystemChange,
                prog.as_str(),
                "changes the network setup of the machine: over ssh this can cut your own connection.".into(),
            );
        }
        "umask" if positional.first().is_some_and(|m| m.chars().all(|c| c == '0')) => {
            warn(
                Risk::GivesAwayAccess,
                "umask",
                "makes every file you create from now on readable and writable by every user.".into(),
            );
        }
        "setcap" => {
            warn(
                Risk::GivesAwayAccess,
                "setcap",
                "gives the program root-like powers (raw network access, binding low ports, ...) without a password."
                    .into(),
            );
        }
        "visudo" => {
            warn(Risk::GivesAwayAccess, "visudo", "edits who may run commands as root: a wrong or too generous line gives a user full control of the machine.".into());
        }
        "passwd" if has(&["-d", "--delete"]) => {
            warn(Risk::WeakensSecurity, "-d", "removes the account's password, so it can log in with none.".into());
        }
        "usermod" | "gpasswd" | "adduser" | "useradd" | "groupmems" => {
            let groups: Vec<String> = {
                let mut g = Vec::new();
                for (i, a) in args.iter().enumerate() {
                    let list = match *a {
                        "-G" | "-aG" | "-Ga" | "--groups" | "-g" | "-a" if prog != "gpasswd" => {
                            args.get(i + 1).copied()
                        }
                        "-a" | "--add" if prog == "gpasswd" => args.get(i + 1).copied(),
                        _ => None,
                    };
                    if let Some(l) = list {
                        g.extend(l.split(',').map(str::to_string));
                        if prog == "gpasswd" {
                            g.extend(args.get(i + 2).map(|s| s.to_string()));
                        }
                    }
                }
                if prog == "adduser" && positional.len() == 2 {
                    g.push(positional[1].to_string());
                }
                g
            };
            if let Some(group) = groups.iter().find(|g| ROOT_EQUIVALENT_GROUPS.contains(&g.as_str())) {
                warn(
                    Risk::GivesAwayAccess,
                    group,
                    format!(
                        "the group `{group}` is as good as root: its members can take over the machine (docker and lxd groups can mount the whole disk; sudo and wheel run anything as root)."
                    ),
                );
            }
        }
        _ => {}
    }
    if prog == "sed"
        && has(&["-i", "--in-place"])
        && (joined.contains("permitrootlogin yes")
            || joined.contains("passwordauthentication yes")
            || joined.contains("permitemptypasswords yes"))
    {
        warn(
            Risk::WeakensSecurity,
            "sshd_config",
            "lets people log in over ssh with a password or as root: far easier to guess or brute-force than a key."
                .into(),
        );
    }

    // --- scheduled and started later -----------------------------------------------------------------
    if prog == "crontab" && has(&["-e"]) {
        warn(Risk::Persists, "-e", "edits the jobs that run on their own schedule; whatever you add keeps running, even after you log out, until you remove it.".into());
    }
    if matches!(prog.as_str(), "at" | "batch") && !has(&["-l", "-d", "-c", "-r", "-V"]) {
        warn(
            Risk::Persists,
            prog.as_str(),
            "schedules a command to run later on its own, when you are not looking.".into(),
        );
    }
    if prog == "loginctl" && sub == "enable-linger" {
        warn(
            Risk::Persists,
            "enable-linger",
            "keeps that user's services running without anyone logged in, and starts them at boot.".into(),
        );
    }

    // --- shared things and production ----------------------------------------------------------------
    let publish_text = |what: &str| {
        format!(
            "uploads {what} to a public registry where others will install it; published versions usually cannot be changed or fully removed, and anything in the folder that is not ignored goes with it."
        )
    };
    match (prog.as_str(), sub) {
        ("cargo", "publish") => warn(Risk::AffectsOthers, "publish", publish_text("this crate")),
        ("twine", "upload") => warn(Risk::AffectsOthers, "upload", publish_text("these files")),
        ("gem", "push") => warn(Risk::AffectsOthers, "push", publish_text("this gem")),
        ("poetry" | "uv", "publish") => warn(Risk::AffectsOthers, "publish", publish_text("this package")),
        ("npm" | "pnpm" | "yarn", "unpublish" | "deprecate") => {
            warn(Risk::AffectsOthers, sub, "changes a package other people depend on in the public registry.".into())
        }
        ("helm", "push") => warn(Risk::AffectsOthers, "push", "uploads the chart to a registry others may use.".into()),
        ("gh", "release") if matches!(positional.get(1), Some(&"create") | Some(&"delete") | Some(&"upload")) => warn(
            Risk::AffectsOthers,
            "release",
            "publishes or removes a release that everyone who follows the repository can see and download.".into(),
        ),
        ("gh", "repo") if positional.get(1) == Some(&"delete") => warn(
            Risk::AffectsOthers,
            "delete",
            "deletes the repository on GitHub, with its issues and pull requests; this cannot be undone.".into(),
        ),
        ("gh", "repo")
            if (positional.get(1) == Some(&"create") && has(&["--public"]))
                || (positional.get(1) == Some(&"edit") && joined.contains("visibility public"))
                || joined.contains("--visibility=public") =>
        {
            warn(Risk::AffectsOthers, "--public", "makes the repository visible to the whole internet.".into())
        }
        ("gh", "gist") if positional.get(1) == Some(&"create") && has(&["--public", "-p"]) => warn(
            Risk::AffectsOthers,
            "--public",
            "publishes the file where anyone can read it; check it holds no secrets.".into(),
        ),
        _ => {}
    }
    if matches!(
        prog.as_str(),
        "aws" | "gsutil" | "gcloud" | "az" | "doctl" | "oci" | "heroku" | "scw" | "linode-cli" | "hcloud" | "openstack"
    ) {
        let destructive_word = |w: &str| {
            matches!(
                w,
                "rm" | "rb"
                    | "rmdir"
                    | "delete"
                    | "remove"
                    | "terminate"
                    | "destroy"
                    | "purge"
                    | "deregister"
                    | "revoke"
                    | "terminate-instances"
            ) || w.starts_with("delete-")
                || w.starts_with("remove-")
                || w.starts_with("terminate-")
                || w.starts_with("destroy-")
        };
        if !has(&["--dryrun", "--dry-run"]) && positional.iter().take(6).any(|w| destructive_word(w)) {
            warn(
                Risk::AffectsOthers,
                positional.iter().take(6).find(|w| destructive_word(w)).copied().unwrap_or(sub),
                "deletes cloud resources for real; other systems may depend on them, and there is often no undo."
                    .into(),
            );
        } else if prog == "aws" && has(&["--delete"]) {
            warn(Risk::DestroysData, "--delete", "removes files at the destination that are not in the source; check which side is which first (--dryrun shows the plan).".into());
        }
    }
    if matches!(
        prog.as_str(),
        "fly"
            | "flyctl"
            | "vercel"
            | "netlify"
            | "wrangler"
            | "firebase"
            | "serverless"
            | "sls"
            | "cdk"
            | "pulumi"
            | "tofu"
            | "terragrunt"
            | "railway"
            | "surge"
    ) && (matches!(sub, "deploy" | "publish" | "up" | "destroy" | "apply" | "remove")
        || has(&["--prod", "--production"]))
    {
        warn(
            Risk::AffectsOthers,
            if has(&["--prod", "--production"]) { "--prod" } else { sub },
            "deploys to, or deletes, something live that real users may be using.".into(),
        );
    }
    if prog == "ansible-playbook" && !has(&["-C", "--check", "--syntax-check", "--list-hosts", "--list-tasks"]) {
        warn(Risk::AffectsOthers, "ansible-playbook", "runs the playbook's tasks on every host it lists, as whoever the inventory says; add --check first to see what would change.".into());
    }
    if matches!(prog.as_str(), "redis-cli")
        && args.iter().any(|a| matches!(a.to_ascii_uppercase().as_str(), "FLUSHALL" | "FLUSHDB"))
    {
        warn(Risk::DestroysData, "FLUSH", "deletes every key in the Redis database at once.".into());
    }
    if matches!(prog.as_str(), "mongo" | "mongosh")
        && (joined.contains("dropdatabase")
            || joined.contains(".drop(")
            || joined.contains("deletemany({})")
            || joined.contains("remove({})"))
    {
        warn(
            Risk::DestroysData,
            "--eval",
            "drops a database or collection, or deletes every document in it, straight away.".into(),
        );
    }
    if matches!(prog.as_str(), "awk" | "gawk" | "mawk") && args.iter().any(|a| a.contains("system(")) {
        warn(
            Risk::RunsUnreadCode,
            "system(",
            "the awk program runs shell commands, and huh does not look inside it: read it before running.".into(),
        );
    }
    if prog == "php" && has(&["-r"]) && joined.contains("system(") {
        warn(
            Risk::RunsUnreadCode,
            "-r",
            "the code given runs shell commands, and huh does not look inside it: read it before running.".into(),
        );
    }
    if prog == "unzip" && has(&["-o"]) {
        warn(Risk::DestroysData, "-o", "overwrites files of the same name without asking.".into());
    }
    if matches!(prog.as_str(), "scp" | "rsync")
        && let Some(dest) = args.last()
        && let Some((_, path)) = dest.split_once(':')
        && ["/etc", "/usr", "/boot", "/bin", "/sbin", "/lib", "/var", "/root", "/opt"]
            .iter()
            .any(|p| path == *p || path.starts_with(&format!("{p}/")))
    {
        warn(
            Risk::BigSystemChange,
            "destination",
            format!("writes into the system folder {path} on another computer, replacing files of the same name."),
        );
    }
}

fn secret_like_name(name: &str) -> bool {
    let u = name.to_ascii_uppercase();
    ["_TOKEN", "_SECRET", "_PASSWORD", "_PASSWD", "_API_KEY", "_ACCESS_KEY", "_SECRET_KEY", "_SECRET_ACCESS_KEY"]
        .iter()
        .any(|s| u.ends_with(s))
        || matches!(u.as_str(), "TOKEN" | "SECRET" | "PASSWORD" | "PGPASSWORD" | "API_KEY")
}

/// A word that is shaped like a well-known service's access token.
fn known_token(word: &str) -> bool {
    let w = word.rsplit(['=', ':', '@']).next().unwrap_or(word);
    let w = w.trim_start_matches("Bearer ").trim_start_matches("token ");
    ["ghp_", "gho_", "ghs_", "ghu_", "github_pat_", "glpat-", "xoxb-", "xoxp-", "sk-ant-", "sk-proj-"]
        .iter()
        .any(|p| w.starts_with(p) && w.len() > p.len() + 8)
        || (w.len() == 20 && w.starts_with("AKIA") && w.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_like_a_shell() {
        assert_eq!(tokens("git -c 'a b' commit \"x y\" z\\ w"), vec!["git", "-c", "a b", "commit", "x y", "z w"]);
        assert_eq!(tokens("ssh-keygen -N ''"), vec!["ssh-keygen", "-N", ""]);
    }

    #[test]
    fn credential_files_are_recognised() {
        for f in ["~/.ssh/id_rsa", "id_ed25519", ".env", "/home/a/.aws/credentials", "server.pem", "~/.kube/config"] {
            assert!(sensitive_file(f), "{f}");
        }
        for f in ["~/.ssh/id_rsa.pub", ".env.example", "notes.txt", "known_hosts"] {
            assert!(!sensitive_file(f), "{f}");
        }
    }

    #[test]
    fn interfaces() {
        assert!(all_interfaces("0.0.0.0"));
        assert!(all_interfaces("0.0.0.0:8080"));
        assert!(!all_interfaces("127.0.0.1"));
        assert!(is_loopback("localhost:8000"));
    }
}
