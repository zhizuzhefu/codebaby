//! 端到端集成测试：mini HTTP server + 真实 CLI 二进制
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

const BIN: &str = env!("CARGO_BIN_EXE_requestbaby");

fn spawn_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            handle(stream);
        }
    });
    port
}

fn handle(mut stream: TcpStream) {
    let mut buf = vec![0u8; 8192];
    let mut n = 0;
    // 读到 header 结束
    loop {
        let read = stream.read(&mut buf[n..]).unwrap_or(0);
        if read == 0 {
            return;
        }
        n += read;
        if find_sub(&buf[..n], b"\r\n\r\n").is_some() {
            break;
        }
    }
    let head = String::from_utf8_lossy(&buf[..n]).into_owned();
    let mut lines = head.split("\r\n");
    let req_line = lines.next().unwrap_or_default().to_string();
    let mut parts = req_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    let mut content_length = 0usize;
    for l in lines {
        if let Some(v) = l.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = v.trim().parse().unwrap_or(0);
        }
    }
    let header_end = find_sub(&buf[..n], b"\r\n\r\n").unwrap() + 4;
    let mut body = buf[header_end..n].to_vec();
    while body.len() < content_length {
        let read = stream.read(&mut buf).unwrap_or(0);
        if read == 0 {
            break;
        }
        body.extend_from_slice(&buf[..read]);
    }
    let body = String::from_utf8_lossy(&body).into_owned();

    let (status, resp_body) = route(&method, &path, &body, &head);
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{resp_body}",
        resp_body.len()
    );
    let _ = stream.write_all(resp.as_bytes());
}

fn route(method: &str, path: &str, body: &str, head: &str) -> (&'static str, String) {
    match (method, path) {
        ("GET", "/health") => ("200 OK", r#"{"status":"ok","version":"1.2.0"}"#.into()),
        ("POST", "/auth/login") => {
            if body.contains("admin") && body.contains("secret") {
                ("200 OK", r#"{"token":"jwt-abc-123","expires_in":3600}"#.into())
            } else {
                ("401 Unauthorized", r#"{"error":"invalid"}"#.into())
            }
        }
        ("GET", "/users/me") => {
            if head.contains("Bearer jwt-abc-123") {
                ("200 OK", r#"{"id":1,"name":"admin","tags":["dev","ops"]}"#.into())
            } else {
                ("401 Unauthorized", r#"{"error":"unauthorized"}"#.into())
            }
        }
        ("POST", "/orders") => ("201 Created", r#"{"data":{"id":42,"sku":"A1"}}"#.into()),
        ("POST", "/echo") => {
            let body_json = serde_json::to_string(&body).unwrap_or_default();
            // 回显关键 headers（小写 key），供集合级 headers 继承断言
            let mut hdrs = serde_json::Map::new();
            for line in head.split("\r\n") {
                if let Some(idx) = line.find(':') {
                    let (k, v) = line.split_at(idx);
                    hdrs.insert(
                        k.trim().to_lowercase(),
                        serde_json::Value::String(v[1..].trim().to_string()),
                    );
                }
            }
            (
                "200 OK",
                format!(r#"{{"body":{body_json},"headers":{}}}"#, serde_json::Value::Object(hdrs)),
            )
        }
        ("GET", "/slow") => {
            thread::sleep(Duration::from_millis(150));
            ("200 OK", r#"{"ok":true}"#.into())
        }
        _ => ("404 Not Found", r#"{"error":"nf"}"#.into()),
    }
}

fn find_sub(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn write_project(dir: &std::path::Path, port: u16) {
    use std::fs;
    fs::create_dir_all(dir.join("envs")).unwrap();
    fs::create_dir_all(dir.join("collections")).unwrap();
    fs::write(
        dir.join("requestbaby.json"),
        r#"{"version":1,"defaultEnv":"test","vars":{"user":"admin","taskId":"738011164633133","flag":"true"},"timeoutMs":5000}"#,
    )
    .unwrap();
    fs::write(
        dir.join("envs/test.json"),
        &format!(r#"{{"vars":{{"baseUrl":"http://127.0.0.1:{port}"}}}}"#),
    )
    .unwrap();
    fs::write(
        dir.join("collections/01-health.json"),
        r#"{
  "tags": ["smoke"],
  "request": { "method": "GET", "url": "{{baseUrl}}/health" },
  "tests": ["res.status === 200", "res.json().version === '1.2.0'"]
}"#,
    )
    .unwrap();
    fs::write(
        dir.join("collections/02-login.json"),
        r#"{
  "tags": ["smoke", "auth"],
  "request": {
    "method": "POST",
    "url": "{{baseUrl}}/auth/login",
    "body": { "json": { "username": "{{user}}", "password": "secret" } }
  },
  "pre": "req.headers['X-Ts'] = '1700000000'",
  "post": "vars.token = res.json().token",
  "tests": ["res.status === 200", "res.json().expires_in === 3600", "vars.token === 'jwt-abc-123'"]
}"#,
    )
    .unwrap();
    fs::write(
        dir.join("collections/03-me.json"),
        r#"{
  "tags": ["auth"],
  "request": {
    "method": "GET",
    "url": "{{baseUrl}}/users/me",
    "headers": { "Authorization": "Bearer {{token}}" }
  },
  "tests": ["res.status === 200", "res.json().tags[0] === 'dev'"]
}"#,
    )
    .unwrap();
    fs::write(
        dir.join("collections/04-slow-fail.json"),
        r#"{
  "tags": ["slow"],
  "request": { "method": "GET", "url": "{{baseUrl}}/slow" },
  "tests": ["res.timeMs < 10"]
}"#,
    )
    .unwrap();
    fs::write(
        dir.join("collections/05-typed-body.json"),
        r#"{
  "tags": ["typed"],
  "request": {
    "method": "POST",
    "url": "{{baseUrl}}/echo",
    "body": { "json": { "taskId": "{{taskId:number}}", "flag": "{{flag:bool}}", "name": "{{user}}" } }
  },
  "tests": [
    "res.json().body === '{\"flag\":true,\"name\":\"admin\",\"taskId\":738011164633133}'"
  ]
}"#,
    )
    .unwrap();
    fs::write(dir.join("collections/collection.json").to_str().unwrap(), r#"{
  "headers": { "x-suite": "inherited", "accept": "application/json" }
}"#).unwrap();
    fs::write(
        dir.join("collections/06-collection-defaults.json"),
        r#"{
  "name": "collection-defaults",
  "tags": ["suite"],
  "expectStatus": 200,
  "request": {
    "method": "POST",
    "url": "{{baseUrl}}/echo",
    "body": { "json": { "ping": 1 } }
  },
  "tests": [
    "data.headers['x-suite'] === 'inherited'",
    "data.headers.accept === 'application/json'"
  ]
}"#,
    )
    .unwrap();
}

fn run_cli(args: &[&str], cwd: &std::path::Path) -> std::process::Output {
    Command::new(BIN)
        .args(args)
        .current_dir(cwd)
        .env_remove("NO_COLOR")
        .output()
        .expect("启动 requestbaby 失败")
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for c2 in chars.by_ref() {
                if c2 == 'm' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[test]
fn full_pass_flow_with_capture_chain() {
    let port = spawn_server();
    let dir = TempDir::new().unwrap();
    write_project(dir.path(), port);

    // 只跑 01~03（skip slow）
    let out = run_cli(&["run", "collections", "--tag", "smoke", "--tag", "auth"], dir.path());
    let stdout = strip_ansi(&String::from_utf8_lossy(&out.stdout));
    assert!(out.status.success(), "应全部通过\nstdout: {stdout}\nstderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("01-health"), "{stdout}");
    assert!(stdout.contains("02-login"), "{stdout}");
    assert!(stdout.contains("3 passed"), "{stdout}");
    // 03-me 依赖 02 捕获的 token（401 会失败），通过说明捕获链生效
    assert!(stdout.contains("03-me"), "{stdout}");
}

#[test]
fn failing_test_reports_and_exit_code() {
    let port = spawn_server();
    let dir = TempDir::new().unwrap();
    write_project(dir.path(), port);
    let out = run_cli(&["run", "collections/04-slow-fail.json"], dir.path());
    let stdout = strip_ansi(&String::from_utf8_lossy(&out.stdout));
    assert!(!out.status.success());
    assert!(stdout.contains("res.timeMs < 10"), "{stdout}");
    assert!(stdout.contains("1 failed"), "{stdout}");
}

#[test]
fn tag_filter_selects_subset() {
    let port = spawn_server();
    let dir = TempDir::new().unwrap();
    write_project(dir.path(), port);
    let out = run_cli(&["run", "collections", "--tag", "auth", "--json"], dir.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());
    assert!(!stdout.contains("01-health"), "{stdout}");
    assert!(stdout.contains("02-login") || stdout.contains("login"), "{stdout}");
    let report: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["total"], 2);
}

#[test]
fn dry_run_prints_curl_without_execution() {
    let dir = TempDir::new().unwrap();
    let port = spawn_server();
    write_project(dir.path(), port);
    let out = run_cli(&["run", "collections", "--dry-run"], dir.path());
    let stdout = strip_ansi(&String::from_utf8_lossy(&out.stdout));
    assert!(out.status.success());
    assert!(stdout.contains("$ curl"), "{stdout}");
    assert!(!stdout.contains("passed,  0 failed") || !stdout.contains("ms"));
}

#[test]
fn validate_catches_bad_config() {
    let dir = TempDir::new().unwrap();
    let port = spawn_server();
    write_project(dir.path(), port);
    // 写一个坏用例：未知字段 + 未定义变量 + 坏脚本
    std::fs::write(
        dir.path().join("collections/99-bad.json"),
        r#"{
  "request": { "method": "GET", "url": "{{baseUrl}}/x?sign={{nope}}" },
  "post": "vars.a = = 1",
  "tests": ["res.status === 200", "res.json(].x"]
}"#,
    )
    .unwrap();
    let out = run_cli(&["validate", "collections"], dir.path());
    let stdout = strip_ansi(&String::from_utf8_lossy(&out.stdout));
    assert!(!out.status.success(), "有 error 应退出码 1\n{stdout}");
    assert!(stdout.contains("nope"), "未定义变量告警\n{stdout}");
    assert!(stdout.contains("post"), "脚本语法错误\n{stdout}");
    assert!(stdout.contains("tests[1]"), "tests 语法错误\n{stdout}");
}

#[test]
fn validate_ok_on_good_project() {
    let dir = TempDir::new().unwrap();
    let port = spawn_server();
    write_project(dir.path(), port);
    let out = run_cli(&["validate", "collections"], dir.path());
    assert!(out.status.success());
}

#[test]
fn strict_parse_unknown_field_fails_at_run() {
    let dir = TempDir::new().unwrap();
    let port = spawn_server();
    write_project(dir.path(), port);
    std::fs::write(
        dir.path().join("collections/98-unknown.json"),
        r#"{ "request": { "method": "GET", "url": "{{baseUrl}}/health" }, "oops": true }"#,
    )
    .unwrap();
    let out = run_cli(&["run", "collections/98-unknown.json"], dir.path());
    assert!(!out.status.success());
    let stdout = strip_ansi(&String::from_utf8_lossy(&out.stdout));
    assert!(stdout.contains("oops"), "未知字段应在报告中呈现\n{stdout}");
}

#[test]
fn typed_number_body_end_to_end() {
    let port = spawn_server();
    let dir = TempDir::new().unwrap();
    write_project(dir.path(), port);
    let out = run_cli(&["run", "collections/05-typed-body.json", "--json"], dir.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    let report: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["passed"], 1, "{stdout}");
}

#[test]
fn collection_headers_and_sugar_end_to_end() {
    let port = spawn_server();
    let dir = TempDir::new().unwrap();
    write_project(dir.path(), port);
    // expectStatus + data 快捷引用 + 集合级 headers 继承
    let out = run_cli(&["run", "collections/06-collection-defaults.json", "--json"], dir.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    let report: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["passed"], 1, "{stdout}");
    // expectStatus 生成的隐式断言应为首条
    let t0 = report["results"][0]["tests"][0]["expr"].as_str().unwrap();
    assert!(t0.contains("includes(res.status)"), "{stdout}");
}

#[test]
fn list_shows_cases() {
    let dir = TempDir::new().unwrap();
    let port = spawn_server();
    write_project(dir.path(), port);
    let out = run_cli(&["list", "collections"], dir.path());
    let stdout = strip_ansi(&String::from_utf8_lossy(&out.stdout));
    assert!(out.status.success());
    assert!(stdout.contains("01-health.json"), "{stdout}");
    assert!(stdout.contains("6 个用例"), "{stdout}");
}
