//! 查看类命令：gRPC / Web / Android / Mac 桌面快照、代理、升级

use super::*;

/// gRPC 用例：尽量取到请求骨架（反射 / .proto），取不到就写空消息并说明原因
pub(crate) fn new_grpc_case(dir: &str, name: &str, method: &str, target: Option<&str>, env: Option<&str>) -> anyhow::Result<PathBuf> {
    let target = target.unwrap_or("{{grpcTarget}}");
    let abs = std::fs::canonicalize(dir).unwrap_or_else(|_| PathBuf::from(dir));
    let template = match session::project_root(&abs).or_else(|| session::project_root(std::path::Path::new("."))) {
        Some(root) => {
            let env = session::resolve_env(std::slice::from_ref(&root), env);
            match protocols::grpc_describe(&root, &env, target, method, false) {
                Ok(info) => Some(info.template),
                Err(e) => {
                    eprintln!("\x1b[33m!\x1b[0m 没取到请求消息骨架，先写空消息：{e:#}");
                    None
                }
            }
        }
        None => None,
    };
    scaffold::new_grpc_case(&PathBuf::from(dir), name, target, method, template)
}

/// `grpc list` / `grpc describe`：查看服务的方法与请求消息骨架（与 Desktop 用例编辑器「加载方法」同一实现）
pub(crate) fn cmd_grpc(action: GrpcCmd) -> u8 {
    let Some(root) = session::project_root(std::path::Path::new(".")) else {
        return err("未找到项目根（codebaby.json）");
    };
    match action {
        GrpcCmd::List { target, env, insecure, json } => {
            let env = session::resolve_env(std::slice::from_ref(&root), env.as_deref());
            match protocols::grpc_methods(&root, &env, &target, insecure) {
                Ok(methods) if json => {
                    println!("{}", serde_json::to_string_pretty(&methods).unwrap_or_default());
                    0
                }
                Ok(methods) => {
                    let w = methods.iter().map(|m| m.method.chars().count()).max().unwrap_or(0);
                    for m in &methods {
                        println!("{:<w$}  \x1b[2m{} · {} → {}\x1b[0m", m.method, m.kind_label(), m.input_type, m.output_type);
                    }
                    if methods.is_empty() {
                        println!("\x1b[2m没有方法\x1b[0m");
                    }
                    0
                }
                Err(e) => err(format!("{e:#}")),
            }
        }
        GrpcCmd::Describe { method, target, env, insecure, json } => {
            let env = session::resolve_env(std::slice::from_ref(&root), env.as_deref());
            match protocols::grpc_describe(&root, &env, &target, &method, insecure) {
                Ok(info) if json => {
                    println!("{}", serde_json::to_string_pretty(&info).unwrap_or_default());
                    0
                }
                Ok(info) => {
                    println!("\x1b[1m{}\x1b[0m \x1b[2m{} · {} → {}\x1b[0m", info.method, info.kind_label(), info.input_type, info.output_type);
                    let key = if info.client_streaming { "messages" } else { "message" };
                    let body = if info.client_streaming { serde_json::json!([info.template]) } else { info.template.clone() };
                    println!("\x1b[2m── 请求消息骨架：粘贴进用例的 \"grpc\": {{ \"{key}\": … }}\x1b[0m");
                    println!("{}", serde_json::to_string_pretty(&body).unwrap_or_default());
                    0
                }
                Err(e) => err(format!("{e:#}")),
            }
        }
    }
}

pub(crate) fn print_snapshot(snap: &protocols::UiSnapshot, json: bool, next: &str) -> u8 {
    if json {
        println!("{}", serde_json::to_string_pretty(snap).unwrap_or_default());
        return 0;
    }
    let (dim, bold, reset) = ("\x1b[2m", "\x1b[1m", "\x1b[0m");
    println!("{bold}{}{reset} {dim}{}{reset}", snap.title, snap.location);
    let width = |s: &str| s.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum::<usize>();
    let labels: Vec<String> = snap.elements.iter().map(|e| format!("{}「{}」", e.kind, e.name.chars().take(24).collect::<String>())).collect();
    let w = labels.iter().map(|l| width(l)).max().unwrap_or(10).min(44);
    for (e, label) in snap.elements.iter().zip(&labels) {
        let pad = w.saturating_sub(width(label));
        let states = if e.states.is_empty() { String::new() } else { format!(" {dim}[{}]{reset}", e.states.join(" ")) };
        println!("  {label}{} {}{states}", " ".repeat(pad), e.locator);
    }
    if snap.elements.is_empty() {
        println!("{dim}  （没有找到可操作的元素）{reset}");
    }
    if let Some(s) = &snap.screenshot {
        println!("{dim}截图 {}{reset}", s.display());
    }
    println!("{dim}{next}{reset}");
    0
}

pub(crate) fn cmd_web(action: WebCmd) -> u8 {
    match action {
        WebCmd::Snapshot { url, env, headed, ocr, json } => {
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let root = session::project_root(&cwd);
            let env = session::resolve_env(std::slice::from_ref(&cwd), env.as_deref());
            match protocols::web_snapshot(root.as_deref(), &env, &url, headed, ocr) {
                Ok(s) => print_snapshot(&s, json, "定位粘贴进步骤：{\"click\": <定位>} / {\"fill\": <定位>, \"value\": \"…\"} / {\"expect\": <定位>}（codebaby guide web）"),
                Err(e) => err(format!("{e:#}")),
            }
        }
    }
}

pub(crate) fn cmd_upgrade(check: bool) -> u8 {
    use codebaby_core::update;
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let root = session::project_root(&cwd);
    let proxy = match protocols::default_proxy(root.as_deref()) {
        Ok(p) => p,
        Err(e) => return fatal(format!("{e:#}")),
    };
    let rel = match update::latest(&proxy) {
        Ok(Some(r)) => r,
        Ok(None) => {
            println!("当前 {}；仓库 {} 还没有发布版本", update::CURRENT, update::repo());
            return 0;
        }
        Err(e) => return err(format!("{e:#}")),
    };
    if !rel.is_newer() {
        println!("\x1b[32m✓\x1b[0m 已是最新版本 {}", update::CURRENT);
        return 0;
    }
    println!("有新版本 \x1b[1m{}\x1b[0m（当前 {}）  {}", rel.version, update::CURRENT, rel.page);
    if !rel.notes.is_empty() {
        for l in rel.notes.lines().take(12) {
            println!("  \x1b[2m{l}\x1b[0m");
        }
    }
    if check {
        return 1;
    }
    let Some(asset) = rel.cli_asset() else {
        return err(format!("这个系统（{} {}）没有现成的命令行安装包：到 {} 下载，或用 cargo install 从源码安装", std::env::consts::OS, std::env::consts::ARCH, rel.page));
    };
    if update::exe_in_app_bundle(&std::env::current_exe().unwrap_or_default()) {
        return upgrade_app_bundle(&rel, &proxy);
    }
    println!("下载 {}（{:.1} MB）…", asset.name, asset.size as f64 / 1_048_576.0);
    let dir = std::env::temp_dir().join(format!("codebaby-download-{}", std::process::id()));
    let result = update::download(&rel, asset, &dir, &proxy).and_then(|archive| update::install_cli(&archive, &rel.version));
    let _ = std::fs::remove_dir_all(&dir);
    match result {
        Ok(exe) => {
            println!("\x1b[32m✓\x1b[0m 已升级到 {}：{}", rel.version, exe.display());
            0
        }
        Err(e) => err(format!("{e:#}")),
    }
}

/// 命令行在 codebaby.app 里：与桌面端同一套自动更新（下载 → 校验 → 退出后原子替换）；命令行随 App 一起更新
pub(crate) fn upgrade_app_bundle(rel: &codebaby_core::update::Release, proxy: &codebaby_core::engine::proxy::ProxyPolicy) -> u8 {
    use codebaby_core::update_app as ua;
    if let Some(why) = ua::current_app().and_then(|a| ua::self_update_blocker(&a)) {
        return err(format!("不能自动更新：{why}；或到 {} 手动下载 DMG", rel.page));
    }
    println!("下载并校验 {}（签名 / 团队 / 公证 / 能运行）…", rel.version);
    let pending = match ua::stage(rel, proxy) {
        Ok(p) => p,
        Err(e) => return err(format!("{e:#}（当前版本没有任何改动）")),
    };
    // 本进程一退出（以及所有 codebaby 窗口都关掉后）更新程序就会替换；等得久一些，给用户时间退出 App
    if let Err(e) = ua::spawn_apply(&pending, false, 900) {
        return err(format!("{e:#}（新版本已暂存，下次打开桌面端退出时会自动安装）"));
    }
    println!("\x1b[32m✓\x1b[0m {} 已下载并校验。退出所有 codebaby 窗口后会自动替换（15 分钟内），失败会保留当前版本", pending.version);
    0
}

pub(crate) fn cmd_proxy(action: Option<ProxyCmd>) -> u8 {
    use codebaby_core::proxy_setting::{self as ps, Mode};
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let Some(root) = session::project_root(&cwd) else { return fatal("不在 codebaby 项目里（没有 codebaby.json）") };
    let current = match ps::read(&root) {
        Ok(c) => c,
        Err(e) => return fatal(format!("{e:#}")),
    };
    let next = match action {
        None => {
            let mode = Mode::of(current.as_ref());
            println!("\x1b[1m代理\x1b[0m  {}", mode.label());
            if let Some(c) = &current {
                if !c.url.is_empty() {
                    println!("  地址    {}", codebaby_core::engine::proxy::redact(&c.url));
                }
                if !c.no_proxy.is_empty() {
                    println!("  直连    {}", c.no_proxy.join(", "));
                }
            }
            if std::env::var(codebaby_core::engine::proxy::ENV_OVERRIDE).is_ok_and(|v| !v.trim().is_empty()) {
                println!("  \x1b[33m本次被 --proxy / --no-proxy / CODEBABY_PROXY 覆盖\x1b[0m");
            }
            let env = session::resolve_env(std::slice::from_ref(&root), None);
            return match protocols::proxy(&root, &env) {
                Ok(policy) => {
                    println!("  生效    {}（环境 {env}）", policy.describe());
                    if let codebaby_core::engine::proxy::ProxyPolicy::On(p) = &policy {
                        match codebaby_core::engine::proxy::probe(p, std::time::Duration::from_secs(5)) {
                            Ok(()) => println!("  \x1b[32m✓ 代理可连接\x1b[0m"),
                            Err(e) => return err(format!("{e:#}")),
                        }
                    }
                    println!("\x1b[2m本机地址（localhost / 127.0.0.1）始终直连；proxy set <地址> / on / off / unset 修改\x1b[0m");
                    0
                }
                Err(e) => err(format!("{e:#}")),
            };
        }
        Some(ProxyCmd::Set { url, bypass }) => {
            let no_proxy = match bypass {
                Some(b) => ps::parse_no_proxy(&b),
                None => current.as_ref().map(|c| c.no_proxy.clone()).unwrap_or_default(),
            };
            ps::build(Mode::On, &url, no_proxy)
        }
        Some(ProxyCmd::On) => match &current {
            Some(c) if !c.url.is_empty() => ps::build(Mode::On, &c.url, c.no_proxy.clone()),
            _ => Err(anyhow::anyhow!("还没有设置代理地址：codebaby proxy set socks5://主机:端口")),
        },
        Some(ProxyCmd::Off) => {
            let c = current.clone().unwrap_or(codebaby_core::config::ProxyConfig { enabled: false, url: String::new(), no_proxy: vec![] });
            ps::build(Mode::Off, &c.url, c.no_proxy)
        }
        Some(ProxyCmd::Unset) => Ok(None),
    };
    let next = match next {
        Ok(n) => n,
        Err(e) => return err(format!("{e:#}")),
    };
    if let Err(e) = ps::write(&root, next.as_ref()) {
        return fatal(format!("{e:#}"));
    }
    let mode = Mode::of(next.as_ref());
    match &next {
        Some(c) if mode == Mode::On => println!("✓ 代理已启用：{}（codebaby.json）", codebaby_core::engine::proxy::redact(&c.url)),
        _ => println!("✓ 代理：{}（codebaby.json）", mode.label()),
    }
    0
}

pub(crate) fn cmd_android(action: AndroidCmd) -> u8 {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let root = session::project_root(&cwd);
    match action {
        AndroidCmd::Devices => match protocols::android_devices(root.as_deref()) {
            Ok(protocols::AndroidDevices { list, problem, via }) => {
                if let Some(v) = &via {
                    eprintln!("\x1b[2m连接：{v}\x1b[0m");
                }
                if let Some(p) = &problem {
                    eprintln!("\x1b[31m{p}\x1b[0m");
                }
                if list.is_empty() {
                    if problem.is_some() {
                        return 1;
                    }
                    return err("没有连接 Android 设备：连接手机并打开 USB 调试，或启动模拟器；远程模拟器 / 云手机用 ANDROID_SERIAL 或 android.serial 写 主机:端口");
                }
                for (id, state) in list {
                    let note = match state.as_str() {
                        "device" => "\x1b[32m可用\x1b[0m",
                        "unauthorized" => "\x1b[33m未授权：在设备上允许调试\x1b[0m",
                        "offline" => "\x1b[31m离线\x1b[0m",
                        _ => state.as_str(),
                    };
                    let remote = if codebaby_core::engine::android::is_network_serial(&id) { "  （网络）" } else { "" };
                    println!("{id}  {note}{remote}");
                }
                u8::from(problem.is_some())
            }
            Err(e) => err(format!("{e:#}")),
        },
        AndroidCmd::Snapshot { ocr, json } => match protocols::android_snapshot(root.as_deref(), ocr) {
            Ok(s) => print_snapshot(&s, json, "定位粘贴进步骤：{\"tap\": <定位>} / {\"fill\": <定位>, \"value\": \"…\"} / {\"expect\": <定位>}（codebaby guide android）"),
            Err(e) => err(format!("{e:#}")),
        },
    }
}

pub(crate) fn cmd_desktop(action: DesktopCmd) -> u8 {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let root = session::project_root(&cwd);
    match action {
        DesktopCmd::Apps => match protocols::desktop_apps(root.as_deref()) {
            Ok(apps) => {
                for (name, bundle) in apps {
                    println!("{name}  \x1b[2m{bundle}\x1b[0m");
                }
                0
            }
            Err(e) => err(format!("{e:#}")),
        },
        DesktopCmd::Snapshot { app, window, ocr, json } => match protocols::desktop_snapshot(root.as_deref(), &app, window.as_deref(), ocr) {
            Ok(s) => print_snapshot(&s, json, "定位粘贴进步骤：{\"click\": <定位>} / {\"fill\": <定位>, \"value\": \"…\"} / {\"expect\": <定位>}（codebaby guide desktop）"),
            Err(e) => err(format!("{e:#}")),
        },
    }
}
