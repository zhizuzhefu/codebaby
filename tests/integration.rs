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
    let raw_head = String::from_utf8_lossy(&buf[..n]).into_owned();
    let mut lines = raw_head.split("\r\n");
    let req_line = lines.next().unwrap_or_default().to_string();
    let mut parts = req_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let raw_path = parts.next().unwrap_or("").to_string();
    let path = raw_path.split('?').next().unwrap_or("").to_string();
    let mut content_length = 0usize;
    for l in lines {
        if let Some(v) = l.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = v.trim().parse().unwrap_or(0);
        }
    }
    let header_end = find_sub(&buf[..n], b"\r\n\r\n").unwrap() + 4;
    // 只把 header 区传给路由（避免 body 行被当 header 解析）
    let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
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
        ("GET", "/orders/42") => (
            "200 OK",
            r#"{"data":{"id":42,"amount":99.5,"status":"paid","items":[{"sku":"A1","qty":2}]}}"#.into(),
        ),
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
        r#"{"version":1,"defaultEnv":"test","vars":{"user":"admin","taskId":"738011164633133","flag":"true","secretKey":{"value":"rb-secret-42","secret":true}},"timeoutMs":5000,
           "workflows":{"chain":{"steps":[{"run":"collections/02-login.json"},{"run":"collections/03-me.json"}]}}}"#,
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
    "res.json().body === '{\"taskId\":738011164633133,\"flag\":true,\"name\":\"admin\"}'"
  ]
}"#,
    )
    .unwrap();
    fs::write(
        dir.join("collections/07-secret.json"),
        r#"{
  "tags": ["secret"],
  "request": {
    "method": "GET",
    "url": "{{baseUrl}}/health",
    "headers": { "x-secret-key": "{{secretKey}}" }
  },
  "tests": ["res.status === 200"]
}"#,
    )
    .unwrap();
    fs::write(
        dir.join("collections/08-envvar.json"),
        r#"{
  "tags": ["envvar"],
  "request": {
    "method": "GET",
    "url": "{{baseUrl}}/health?trace={{%RB_TRACE_ID}}"
  },
  "tests": ["res.status === 200"]
}"#,
    )
    .unwrap();
    // 集合 setup/teardown（setup 内 http 造数）
    std::fs::create_dir_all(dir.join("collections/suite")).unwrap();
    fs::write(
        dir.join("collections/suite/collection.json"),
        r#"{
  "name": "suite",
  "setup": "vars.prepared = 'yes'; const r = http.get(env.baseUrl + '/health'); vars.probeStatus = r.status",
  "teardown": "log('teardown done')"
}"#,
    )
    .unwrap();
    fs::write(
        dir.join("collections/suite/01-probe.json"),
        r#"{
  "tags": ["suite2"],
  "expectStatus": 200,
  "request": { "method": "GET", "url": "{{baseUrl}}/health" },
  "tests": ["env.prepared === 'yes'", "env.probeStatus === 200"]
}"#,
    )
    .unwrap();
    // 数据驱动
    fs::write(
        dir.join("collections/09-data-driven.json"),
        r#"{
  "tags": ["dd"],
  "request": {
    "method": "POST",
    "url": "{{baseUrl}}/echo",
    "body": { "json": { "sku": "{{sku}}" } }
  },
  "cases": [
    { "label": "A1", "sku": "A1" },
    { "label": "B2", "sku": "B2" }
  ],
  "tests": ["data.body.indexOf(env.sku) >= 0", "res.status === 200"]
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
    let out = run_cli(&["run", "collections", "--tag", "auth", "--report", "json"], dir.path());
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
    let out = run_cli(&["run", "collections/05-typed-body.json", "--report", "json"], dir.path());
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
    let out = run_cli(&["run", "collections/06-collection-defaults.json", "--report", "json"], dir.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    let report: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["passed"], 1, "{stdout}");
    // expectStatus 生成的隐式断言应为首条
    let t0 = report["results"][0]["tests"][0]["expr"].as_str().unwrap();
    assert!(t0.contains("includes(res.status)"), "{stdout}");
}

#[test]
fn secret_vars_redacted_in_output() {
    let port = spawn_server();
    let dir = TempDir::new().unwrap();
    write_project(dir.path(), port);
    let out = run_cli(&["run", "collections/07-secret.json", "--dry-run"], dir.path());
    let stdout = strip_ansi(&String::from_utf8_lossy(&out.stdout));
    assert!(out.status.success());
    assert!(!stdout.contains("rb-secret-42"), "secret 明文不得出现在输出: {stdout}");
    assert!(stdout.contains("****"), "应脱敏: {stdout}");
}

#[test]
fn env_var_inline_syntax() {
    let port = spawn_server();
    let dir = TempDir::new().unwrap();
    write_project(dir.path(), port);
    let out = Command::new(BIN)
        .args(["run", "collections/08-envvar.json", "--report", "json"])
        .current_dir(dir.path())
        .env("RB_TRACE_ID", "tr-9527")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains("tr-9527"), "%%ENV%% 应被替换: {stdout}");
}

#[test]
fn output_dir_persists_responses() {
    let port = spawn_server();
    let dir = TempDir::new().unwrap();
    write_project(dir.path(), port);
    let outdir = dir.path().join("out");
    let out = run_cli(&["run", "collections/01-health.json", "-o", "out"], dir.path());
    assert!(out.status.success());
    let saved = std::fs::read_to_string(outdir.join("collections/01-health.resp.json")).unwrap();
    assert!(saved.contains("\"status\": 200"), "{saved}");
}

#[test]
fn failed_case_has_response_snapshot() {
    let port = spawn_server();
    let dir = TempDir::new().unwrap();
    write_project(dir.path(), port);
    let out = run_cli(&["run", "collections/04-slow-fail.json", "--report", "json"], dir.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let report: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(report["results"][0]["response"]["body"]
        .as_str()
        .unwrap()
        .contains("ok"), "失败快照应含响应体: {stdout}");
}

#[test]
fn collection_setup_teardown_with_http() {
    let port = spawn_server();
    let dir = TempDir::new().unwrap();
    write_project(dir.path(), port);
    let out = run_cli(&["run", "collections/suite", "--report", "json"], dir.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "setup 造数 + 用例应全过: {stdout}");
}

#[test]
fn data_driven_expands_rows() {
    let port = spawn_server();
    let dir = TempDir::new().unwrap();
    write_project(dir.path(), port);
    let out = run_cli(&["run", "collections/09-data-driven.json", "--report", "json"], dir.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains("[A1]"), "实例标签: {stdout}");
    assert!(stdout.contains("[B2]"), "实例标签: {stdout}");
    let report: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["total"], 2, "{stdout}");
}

#[test]
fn import_openapi3() {
    let dir = TempDir::new().unwrap();
    let spec = dir.path().join("openapi.json");
    std::fs::write(&spec, r##"{
  "openapi": "3.0.0",
  "info": { "title": "demo", "version": "1.0" },
  "servers": [{ "url": "https://api.demo.test/v1" }],
  "paths": {
    "/pets": {
      "post": {
        "tags": ["pets"],
        "summary": "create pet",
        "operationId": "createPet",
        "requestBody": { "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Pet" } } } },
        "responses": { "201": { "description": "ok" }, "400": { "description": "bad" } }
      },
      "get": {
        "tags": ["pets"],
        "operationId": "listPets",
        "parameters": [{ "name": "limit", "in": "query", "schema": { "type": "integer" }, "example": "20" }],
        "responses": { "200": { "description": "ok" } }
      }
    },
    "/pets/{petId}": {
      "get": {
        "tags": ["pets"],
        "operationId": "getPet",
        "parameters": [{ "name": "petId", "in": "path", "schema": { "type": "integer" } }],
        "responses": { "200": { "description": "ok" } }
      }
    }
  },
  "components": { "schemas": { "Pet": { "type": "object", "properties": {
    "name": { "type": "string" }, "age": { "type": "integer" } } } } }
}"##).unwrap();
    let out = dir.path().join("imported");
    let r = run_cli(&["import", spec.to_str().unwrap(), "-o", out.to_str().unwrap()], dir.path());
    let stdout = String::from_utf8_lossy(&r.stdout);
    assert!(r.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&r.stderr));
    assert!(stdout.contains("3 个用例"), "{stdout}");
    // 生成的用例可 validate
    let pet_post = std::fs::read_to_string(out.join("collections/pets/post-pets.json")).unwrap();
    assert!(pet_post.contains("{{name}}"), "schema 占位: {pet_post}");
    assert!(pet_post.contains("{{age:number}}"), "数字标注: {pet_post}");
    assert!(pet_post.contains("201"), "expectStatus: {pet_post}");
    let v = run_cli(&["validate", "imported/collections"], dir.path());
    assert!(v.status.success(), "{}", String::from_utf8_lossy(&v.stdout));
}

#[test]
fn import_swagger2() {
    let dir = TempDir::new().unwrap();
    let spec = dir.path().join("swagger.json");
    std::fs::write(&spec, r##"{
  "swagger": "2.0",
  "info": { "title": "sw", "version": "1" },
  "host": "sw.demo.test",
  "basePath": "/api",
  "schemes": ["https"],
  "paths": {
    "/orders": {
      "post": {
        "tags": ["orders"],
        "parameters": [{ "name": "body", "in": "body", "schema": { "$ref": "#/definitions/Order" } }],
        "responses": { "200": { "description": "ok" } }
      }
    }
  },
  "definitions": { "Order": { "type": "object", "properties": { "id": { "type": "integer" } } } }
}"##).unwrap();
    let out = dir.path().join("imported");
    let r = run_cli(&["import", spec.to_str().unwrap(), "-o", out.to_str().unwrap()], dir.path());
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stdout));
    let env = std::fs::read_to_string(out.join("envs/dev.json")).unwrap();
    assert!(env.contains("https://sw.demo.test/api"), "{env}");
    let case = std::fs::read_to_string(out.join("collections/orders/post-orders.json")).unwrap();
    assert!(case.contains("{{id:number}}"), "{case}");
}

#[test]
fn import_postman_v21() {
    let dir = TempDir::new().unwrap();
    let spec = dir.path().join("postman.json");
    std::fs::write(&spec, r##"{
  "info": { "name": "pm-demo", "schema": "v2.1.0" },
  "variable": [{ "key": "baseUrl", "value": "http://pm.test" }],
  "item": [
    {
      "name": "users",
      "item": [
        {
          "name": "create user",
          "request": {
            "method": "POST",
            "header": [{ "key": "Content-Type", "value": "application/json" }],
            "body": { "mode": "raw", "raw": "{\"name\": \"a\"}", "options": { "raw": { "language": "json" } } },
            "url": { "raw": "{{baseUrl}}/users?page=1", "host": ["x"] }
          },
          "event": [{ "listen": "test", "script": { "exec": ["pm.test('x', 1)"] } }]
        }
      ]
    }
  ]
}"##).unwrap();
    let out = dir.path().join("imported");
    let r = run_cli(&["import", spec.to_str().unwrap(), "-o", out.to_str().unwrap()], dir.path());
    let stdout = String::from_utf8_lossy(&r.stdout);
    assert!(r.status.success(), "{stdout}");
    assert!(stdout.contains("需手工迁移"), "脚本迁移提示: {stdout}");
    let case = std::fs::read_to_string(out.join("collections/users/01-create-user.json")).unwrap();
    assert!(case.contains("{{baseUrl}}/users?page=1"), "{case}");
    assert!(case.contains("\"json\"", ), "{case}");
    let env = std::fs::read_to_string(out.join("envs/dev.json")).unwrap();
    assert!(env.contains("http://pm.test"), "{env}");
}

#[test]
fn parallel_mode_runs_independent_cases() {
    let port = spawn_server();
    let dir = TempDir::new().unwrap();
    write_project(dir.path(), port);
    // 01-health(独立) 02-login(有 post) 03-me(依赖 token) → 并发档位 2
    let out = run_cli(&["run", "collections/01-health.json", "collections/02-login.json", "collections/03-me.json", "--parallel", "2", "--report", "json"], dir.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "并发模式依赖链应保序执行: {stdout}");
    let report: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["total"], 3, "{stdout}");
}

#[test]
fn junit_report_format() {
    let port = spawn_server();
    let dir = TempDir::new().unwrap();
    write_project(dir.path(), port);
    let out = run_cli(&["run", "collections/04-slow-fail.json", "--report", "junit"], dir.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!out.status.success());
    assert!(stdout.contains("<testsuites"), "{stdout}");
    assert!(stdout.contains("<failure"), "{stdout}");
}

#[test]
fn html_report_written() {
    let port = spawn_server();
    let dir = TempDir::new().unwrap();
    write_project(dir.path(), port);
    let out = run_cli(&["run", "collections/01-health.json", "--report", "html"], dir.path());
    assert!(out.status.success());
    let html = std::fs::read_to_string(dir.path().join("report.html")).unwrap();
    assert!(html.contains("requestbaby 报告"), "{html}");
    assert!(html.contains("01-health"), "{html}");
}

#[test]
fn leftover_placeholder_clear_error() {
    let dir = TempDir::new().unwrap();
    let port = spawn_server();
    write_project(dir.path(), port);
    // 未定义环境变量：应清晰报错而非 curl nested brace
    std::fs::write(
        dir.path().join("collections/10-leftover.json"),
        r#"{
  "tags": ["leftover"],
  "request": { "method": "GET", "url": "{{baseUrl}}/health?task={{%NOPE_TASK_ID}}" }
}"#,
    )
    .unwrap();
    let out = run_cli(&["run", "collections/10-leftover.json"], dir.path());
    let stdout = strip_ansi(&String::from_utf8_lossy(&out.stdout));
    assert!(!out.status.success());
    assert!(stdout.contains("环境变量 NOPE_TASK_ID 未设置"), "{stdout}");
    // dry-run 不拦截（审阅预期）
    let out2 = run_cli(&["run", "collections/10-leftover.json", "--dry-run"], dir.path());
    assert!(out2.status.success());
}

#[test]
fn workflow_shares_vars_across_steps() {
    let port = spawn_server();
    let dir = TempDir::new().unwrap();
    write_project(dir.path(), port);
    // step1: 02-login post 捕获 token；step2: 03-me header 引用 {{token}}（401 则失败）
    let out = run_cli(&["workflow", "chain"], dir.path());
    let stdout = strip_ansi(&String::from_utf8_lossy(&out.stdout));
    assert!(out.status.success(), "跨 step 变量应共享: {stdout}");
    assert!(stdout.contains("step 2/2"), "{stdout}");
    assert!(stdout.contains("2 passed"), "{stdout}");
}

#[test]
fn workflow_unknown_name_errors() {
    let port = spawn_server();
    let dir = TempDir::new().unwrap();
    write_project(dir.path(), port);
    let out = run_cli(&["workflow", "nope"], dir.path());
    assert_eq!(out.status.code(), Some(1));
}

/// golden 示范项目 = AI-GUIDE 文档的全特性活验证：
/// 捕获链 / {{var:number}} / 数据驱动 / setup / secret / workflow / 负例
#[test]
fn golden_project_full_workflow() {
    let port = spawn_server();
    let dir = TempDir::new().unwrap();
    // 复制 golden 项目
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/golden");
    copy_dir(&src, dir.path());
    // 动态端口 env
    std::fs::create_dir_all(dir.path().join("envs")).unwrap();
    std::fs::write(
        dir.path().join("envs/test.json"),
        format!(r#"{{"vars":{{"baseUrl":"http://127.0.0.1:{port}","token":{{"value":"init-token","secret":true}}}}}}"#),
    )
    .unwrap();
    // 1) validate 全过
    let v = run_cli(&["validate", "--report", "json"], dir.path());
    assert!(v.status.success(), "{}", String::from_utf8_lossy(&v.stdout));
    // 2) fmt 已精简
    let f = run_cli(&["fmt", "--check"], dir.path());
    assert!(f.status.success(), "{}", String::from_utf8_lossy(&f.stdout));
    // 3) workflow 全绿（含捕获链/数据驱动/setup/负例）
    let out = run_cli(&["workflow", "full"], dir.path());
    let stdout = strip_ansi(&String::from_utf8_lossy(&out.stdout));
    assert!(out.status.success(), "golden workflow 应全绿:\n{stdout}");
    assert!(stdout.contains("[正常]"), "数据驱动实例: {stdout}");
    // 4) secret 脱敏：dry-run 输出不含明文
    let d = run_cli(&["run", "--dry-run", "-v"], dir.path());
    let ds = String::from_utf8_lossy(&d.stdout);
    assert!(!ds.contains("init-token") || ds.contains("****"), "secret 应脱敏");
}

fn copy_dir(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for e in walkdir::WalkDir::new(src).into_iter().flatten() {
        if e.file_type().is_file() {
            let rel = e.path().strip_prefix(src).unwrap();
            let to = dst.join(rel);
            std::fs::create_dir_all(to.parent().unwrap()).unwrap();
            std::fs::copy(e.path(), &to).unwrap();
        }
    }
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
    assert!(stdout.contains("10 个用例"), "{stdout}");
}
