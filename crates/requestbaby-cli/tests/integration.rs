//! CLI 集成测试：真实二进制 + 临时项目目录，只覆盖不发出网络请求的命令（校验 / dry-run / 导入 / 脚手架 / 变量）
use std::process::Command;
use std::path::Path;
use tempfile::TempDir;

const BIN: &str = env!("CARGO_BIN_EXE_requestbaby");

fn write_project(dir: &std::path::Path) {
    use std::fs;
    fs::create_dir_all(dir.join("envs")).unwrap();
    fs::create_dir_all(dir.join("collections")).unwrap();
    fs::write(
        dir.join("requestbaby.json"),
        r#"{"version":1,"defaultEnv":"test","vars":{"user":"admin","taskId":"123456789012345","flag":"true","secretKey":{"value":"rb-secret-42","secret":true}},"timeoutMs":5000,
           "workflows":{"chain":{"steps":[{"run":"collections/02-login.json"},{"run":"collections/03-me.json"}]}}}"#,
    )
    .unwrap();
    fs::write(
        dir.join("envs/test.json"),
        r#"{"vars":{"baseUrl":"http://127.0.0.1:9"}}"#,
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
    "res.json().body === '{\"taskId\":123456789012345,\"flag\":true,\"name\":\"admin\"}'"
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
fn dry_run_prints_curl_without_execution() {
    let dir = TempDir::new().unwrap();
    write_project(dir.path());
    let out = run_cli(&["run", "collections", "--dry-run"], dir.path());
    let stdout = strip_ansi(&String::from_utf8_lossy(&out.stdout));
    assert!(out.status.success());
    assert!(stdout.contains("$ curl"), "{stdout}");
    assert!(!stdout.contains("passed,  0 failed") || !stdout.contains("ms"));
}

#[test]
fn validate_catches_bad_config() {
    let dir = TempDir::new().unwrap();
    write_project(dir.path());
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
    write_project(dir.path());
    let out = run_cli(&["validate", "collections"], dir.path());
    assert!(out.status.success());
}

#[test]
fn strict_parse_unknown_field_fails_at_run() {
    let dir = TempDir::new().unwrap();
    write_project(dir.path());
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
fn secret_vars_redacted_in_output() {
    let dir = TempDir::new().unwrap();
    write_project(dir.path());
    let out = run_cli(&["run", "collections/07-secret.json", "--dry-run"], dir.path());
    let stdout = strip_ansi(&String::from_utf8_lossy(&out.stdout));
    assert!(out.status.success());
    assert!(!stdout.contains("rb-secret-42"), "secret 明文不得出现在输出: {stdout}");
    assert!(stdout.contains("****"), "应脱敏: {stdout}");
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
fn leftover_placeholder_clear_error() {
    let dir = TempDir::new().unwrap();
    write_project(dir.path());
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

/// 流程不存在 = 配置错误：exit 2（与「用例失败」的 1 区分，见 AI-NATIVE 退出码语义）
#[test]
fn workflow_unknown_name_errors() {
    let dir = TempDir::new().unwrap();
    write_project(dir.path());
    let out = run_cli(&["workflow", "nope"], dir.path());
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn list_shows_cases() {
    let dir = TempDir::new().unwrap();
    write_project(dir.path());
    let out = run_cli(&["list", "collections"], dir.path());
    let stdout = strip_ansi(&String::from_utf8_lossy(&out.stdout));
    assert!(out.status.success());
    assert!(stdout.contains("01-health.json"), "{stdout}");
    assert!(stdout.contains("10 个用例"), "{stdout}");
}

#[test]
fn new_case_collection_env_use_shared_templates() {
    let dir = TempDir::new().unwrap();
    assert!(run_cli(&["init"], dir.path()).status.success());
    let c = run_cli(&["new", "collection", "collections", "orders"], dir.path());
    assert!(c.status.success(), "{}", String::from_utf8_lossy(&c.stderr));
    let n = run_cli(&["new", "case", "collections/orders", "Create Order", "-m", "post"], dir.path());
    assert!(n.status.success(), "{}", String::from_utf8_lossy(&n.stderr));
    assert!(dir.path().join("collections/orders/01-create-order.json").exists());
    let e = run_cli(&["new", "env", "staging", "--from", "dev"], dir.path());
    assert!(e.status.success(), "{}", String::from_utf8_lossy(&e.stderr));
    let staging = std::fs::read_to_string(dir.path().join("envs/staging.json")).unwrap();
    assert!(staging.contains("localhost:8080"), "{staging}");
    // 新建产物本身就是规范形态
    assert!(run_cli(&["fmt", "--check"], dir.path()).status.success());
}

#[test]
fn vars_preview_reports_sources_and_runtime_captures() {
    let dir = TempDir::new().unwrap();
    write_project(dir.path());
    let out = run_cli(&["vars", "collections/03-me.json", "--json"], dir.path());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    let base = v["entries"].as_array().unwrap().iter().find(|e| e["name"] == "baseUrl").unwrap();
    assert!(base["source"].as_str().unwrap().starts_with("环境"), "{base}");
    assert!(v["runtime"].as_array().unwrap().iter().any(|r| r["name"] == "token"), "{v}");
}

#[test]
fn doctor_json_lists_checks() {
    let dir = TempDir::new().unwrap();
    write_project(dir.path());
    let out = run_cli(&["doctor", "--json"], dir.path());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert!(v["checks"].as_array().unwrap().len() >= 3, "{v}");
}

#[test]
fn junit_format_is_rejected() {
    let dir = TempDir::new().unwrap();
    let out = run_cli(&["run", "--report", "junit"], dir.path());
    assert!(!out.status.success());
}

#[test]
fn vars_set_unset_and_matrix_share_core_varstore() {
    let dir = TempDir::new().unwrap();
    assert!(run_cli(&["init", "."], dir.path()).status.success());

    let out = run_cli(&["vars", "set", "baseUrl", "http://staging", "-e", "prod"], dir.path());
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(run_cli(&["vars", "set", "apiKey", "k-123", "--project", "--secret"], dir.path()).status.success());
    // 层级三选一必填
    assert!(!run_cli(&["vars", "set", "x", "1"], dir.path()).status.success());

    let prod = std::fs::read_to_string(dir.path().join("envs/prod.json")).unwrap();
    assert!(prod.contains("http://staging"), "{prod}");
    let project = std::fs::read_to_string(dir.path().join("requestbaby.json")).unwrap();
    assert!(project.contains("\"secret\": true"), "{project}");

    let out = run_cli(&["vars", "--json"], dir.path());
    let m: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    let labels: Vec<&str> = m["columns"].as_array().unwrap().iter().map(|c| c["label"].as_str().unwrap()).collect();
    assert_eq!(labels, ["项目", "环境 dev", "环境 prod"]);
    let api = m["rows"].as_array().unwrap().iter().find(|r| r["name"] == "apiKey").unwrap();
    assert_eq!(api["cells"][0]["type"], "secret");

    assert!(run_cli(&["vars", "unset", "apiKey", "--project"], dir.path()).status.success());
    assert!(!run_cli(&["vars", "unset", "apiKey", "--project"], dir.path()).status.success(), "已删除再删应报错");

    // 用例预览：解析后的请求 URL
    let out = run_cli(&["vars", "collections/demo/01-health.json", "-e", "prod"], dir.path());
    assert!(String::from_utf8_lossy(&out.stdout).contains("GET http://staging/health"), "{}", String::from_utf8_lossy(&out.stdout));
}

/// 文档（guide 全部主题 + README）里出现的每条命令都必须真实存在：子命令逐级用 --help 验证
#[test]
fn guide_mentions_only_real_commands() {
    let dir = TempDir::new().unwrap();
    let out = run_cli(&["guide", "all"], dir.path());
    assert!(out.status.success());
    let readme = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../README.md")).unwrap();
    let text = format!("{}\n{readme}", String::from_utf8_lossy(&out.stdout));
    // 命令组的子命令清单（取自 --help 的 Commands 段）
    let subcommands = |group: &str| -> Vec<String> {
        let o = run_cli(&[group, "--help"], dir.path());
        let help = String::from_utf8_lossy(&o.stdout).to_string();
        help.split("Commands:")
            .nth(1)
            .map(|c| {
                c.lines()
                    .skip(1)
                    .take_while(|l| !l.trim().is_empty())
                    .filter_map(|l| l.split_whitespace().next().map(String::from))
                    .filter(|w| w != "help")
                    .collect()
            })
            .unwrap_or_default()
    };
    let word = |w: &str| w.trim_end_matches(['`', '）', ')', '；', '，', '。', '"']).to_string();
    let mut cmds: Vec<Vec<String>> = vec![];
    for rest in text.split("requestbaby ").skip(1) {
        let words: Vec<String> = rest.split_whitespace().take(2).map(word).collect();
        let Some(first) = words.first().filter(|f| !f.is_empty() && f.chars().all(|c| c.is_ascii_lowercase())) else { continue };
        let sub = words.get(1).filter(|w| !w.is_empty() && w.chars().all(|c| c.is_ascii_lowercase()));
        let mut cmd = vec![first.clone()];
        if let Some(sub) = sub {
            if matches!(first.as_str(), "new" | "env" | "case" | "collection" | "workflow" | "history" | "vars") {
                let subs = subcommands(first);
                if subs.contains(sub) {
                    cmd.push(sub.clone());
                } else if matches!(first.as_str(), "new" | "env" | "case" | "collection") {
                    panic!("文档引用了不存在的子命令: requestbaby {first} {sub}（可用：{subs:?}）");
                }
            }
            // guide <主题> 必须是真实主题
            if first == "guide" {
                let o = run_cli(&["guide", sub], dir.path());
                assert!(o.status.success(), "文档引用了不存在的主题: requestbaby guide {sub}");
            }
        }
        cmds.push(cmd);
    }
    cmds.sort();
    cmds.dedup();
    assert!(cmds.len() >= 20, "{cmds:?}");
    for cmd in cmds {
        let mut args: Vec<&str> = cmd.iter().map(String::as_str).collect();
        args.push("--help");
        let o = run_cli(&args, dir.path());
        assert!(o.status.success(), "文档引用了不存在的命令: requestbaby {}", cmd.join(" "));
    }
    // 未知主题：exit 2 并列出可选主题
    let o = run_cli(&["guide", "nope"], dir.path());
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("expect"));
}

/// 文档里的示例都是能跑的：每个主题（与 README）的 `json case` / `json collection` / `json project` 块组成一个临时项目，
/// 必须 validate 零错误零警告、run --dry-run 成功；`json fragment` 是片段，至少要是合法 JSON；不允许未标注的 json 块
#[test]
fn guide_examples_validate_and_dry_run() {
    let docs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/guide");
    let mut checked = 0;
    let mut files: Vec<_> = std::fs::read_dir(&docs).unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "md")).collect();
    files.sort();
    files.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../README.md"));
    for doc in files {
        let name = doc.file_stem().unwrap().to_string_lossy().to_string();
        let text = std::fs::read_to_string(&doc).unwrap();
        // 代码块：(信息串, 内容)
        let mut blocks: Vec<(String, String)> = vec![];
        let mut cur: Option<(String, String)> = None;
        for line in text.lines() {
            match (&mut cur, line.trim_start().strip_prefix("```")) {
                (None, Some(info)) => cur = Some((info.trim().to_string(), String::new())),
                (Some(_), Some(_)) => blocks.push(cur.take().unwrap()),
                (Some((_, body)), None) => {
                    body.push_str(line);
                    body.push('\n');
                }
                (None, None) => {}
            }
        }
        let dir = TempDir::new().unwrap();
        let d = dir.path();
        let col = d.join("collections/doc");
        std::fs::create_dir_all(&col).unwrap();
        std::fs::create_dir_all(d.join("envs")).unwrap();
        let mut project = serde_json::json!({"version": 1, "defaultEnv": "dev"});
        let mut sources = String::new();
        let mut n = 0;
        for (info, body) in &blocks {
            let parse = |b: &str| serde_json::from_str::<serde_json::Value>(b).unwrap_or_else(|e| panic!("{name}.md 的 {info} 块不是合法 JSON: {e}\n{b}"));
            match info.as_str() {
                "json case" => {
                    n += 1;
                    let v = parse(body);
                    // guide 只展示规范写法：界面用例部分经 fmt 规范化后不变（Android 写 tap、Web / Mac 写 click、时长写人话）
                    let canon = requestbaby_core::fmt::canonical_case(&v);
                    for key in ["web", "android", "desktop"] {
                        assert_eq!(canon[key], v[key], "{name}.md 的示例不是规范写法（requestbaby fmt 会改成左边那样）：\n{body}");
                    }
                    std::fs::write(col.join(format!("{n:02}-example.json")), body).unwrap();
                    sources.push_str(body);
                }
                "json collection" => {
                    parse(body);
                    std::fs::write(col.join("collection.json"), body).unwrap();
                    sources.push_str(body);
                }
                "json project" => {
                    project = parse(body);
                    // 流程引用的目录 / 环境在临时项目里补齐
                    for wf in project["workflows"].as_object().into_iter().flat_map(|w| w.values()) {
                        for step in wf["steps"].as_array().into_iter().flatten() {
                            std::fs::create_dir_all(d.join(step["run"].as_str().unwrap())).unwrap();
                        }
                        if let Some(env) = wf["env"].as_str() {
                            std::fs::write(d.join(format!("envs/{env}.json")), r#"{"vars":{"baseUrl":"http://127.0.0.1:9"}}"#).unwrap();
                        }
                    }
                }
                "json fragment" => {
                    parse(&format!("{{{body}}}"));
                }
                i if i == "json" || i.starts_with("json ") => panic!("{name}.md 有未标注用途的 json 块（case / collection / project / fragment）：\n{body}"),
                _ => continue,
            }
            checked += 1;
        }
        if n == 0 {
            continue;
        }
        // 示例里引用、但不由 capture / 脚本写入的变量：在环境里给个值
        let captured: Vec<String> = std::fs::read_dir(&col)
            .unwrap()
            .flatten()
            .filter_map(|e| serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(e.path()).ok()?).ok())
            .flat_map(|v| v["capture"].as_object().map(|c| c.keys().cloned().collect::<Vec<_>>()).unwrap_or_default())
            .collect();
        let mut vars = serde_json::Map::new();
        vars.insert("baseUrl".into(), "http://127.0.0.1:9".into());
        // {{x}} 占位符 + 脚本里读的 env.x
        let script_reads = sources.split("env.").skip(1).map(|p| p.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect::<String>());
        let placeholders = sources.split("{{").skip(1).map(|p| p.split("}}").next().unwrap().split(':').next().unwrap().trim().to_string());
        for var in placeholders.chain(script_reads).collect::<Vec<_>>() {
            let written = sources.contains(&format!("vars.{var} ="));
            if !var.is_empty() && !var.starts_with('%') && !captured.contains(&var) && !written && !vars.contains_key(&var) {
                vars.insert(var, "1".into());
            }
        }
        std::fs::write(d.join("envs/dev.json"), serde_json::json!({ "vars": vars }).to_string()).unwrap();
        std::fs::write(d.join("requestbaby.json"), project.to_string()).unwrap();

        let out = run_cli(&["validate", "--report", "json"], d);
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["issues"], serde_json::json!([]), "{name}.md 的示例没有通过 validate：{v:#}");
        let out = run_cli(&["run", "collections", "--dry-run"], d);
        assert!(out.status.success(), "{name}.md 的示例 dry-run 失败：{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    }
    assert!(checked >= 25, "执行的示例太少：{checked}");
}

#[test]
fn init_and_guide_init_write_agent_entry_without_overwriting() {
    let dir = TempDir::new().unwrap();
    assert!(run_cli(&["init"], dir.path()).status.success());
    let agents = std::fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
    assert!(agents.contains("requestbaby guide"));
    assert_eq!(std::fs::read_to_string(dir.path().join("CLAUDE.md")).unwrap(), "@AGENTS.md\n");
    // 已有项目补入口：用户自己的文件不覆盖
    std::fs::write(dir.path().join("AGENTS.md"), "custom\n").unwrap();
    std::fs::remove_file(dir.path().join("CLAUDE.md")).unwrap();
    let out = run_cli(&["guide", "--init"], dir.path().join("collections").as_path());
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(std::fs::read_to_string(dir.path().join("AGENTS.md")).unwrap(), "custom\n");
    assert!(dir.path().join("CLAUDE.md").exists());
}

/// verify 用的项目：被测服务 = 测试内回显（requestbaby 不负责启动服务）
fn write_verify_project(dir: &std::path::Path, port: u16) {
    std::fs::create_dir_all(dir.join("envs")).unwrap();
    std::fs::create_dir_all(dir.join("collections/site")).unwrap();
    std::fs::write(
        dir.join("requestbaby.json"),
        r#"{"version":1,"defaultEnv":"local","workflows":{"smoke":{"steps":[{"run":"collections/site"}]}}}"#,
    )
    .unwrap();
    std::fs::write(dir.join("envs/local.json"), format!(r#"{{"vars":{{"baseUrl":"http://127.0.0.1:{port}"}}}}"#)).unwrap();
    std::fs::write(
        dir.join("collections/site/01-health.json"),
        r#"{"request":{"method":"GET","url":"{{baseUrl}}/health"},"tests":["res.status === 200","res.text().includes('GET /health')"]}"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("collections/site/02-rows.json"),
        r#"{"request":{"method":"GET","url":"{{baseUrl}}/health?n={{n}}"},
            "cases":[{"label":"零","n":0},{"label":"大","n":12}],
            "tests":["typeof env.n === 'number'"]}"#,
    )
    .unwrap();
}

#[test]
fn verify_runs_everything_with_one_verdict() {
    let dir = TempDir::new().unwrap();
    write_verify_project(dir.path(), spawn_echo());
    let out = run_cli(&["verify", "--report", "json"], dir.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let stages: Vec<(&str, &str)> =
        v["steps"].as_array().unwrap().iter().map(|s| (s["label"].as_str().unwrap(), s["status"].as_str().unwrap())).collect();
    assert_eq!(stages, [("校验", "passed"), ("全部用例", "passed"), ("流程 smoke", "passed")]);
    // 全部用例 3 + 流程 3；数据行保留数字类型
    assert_eq!((v["total"].as_u64(), v["passed"].as_u64()), (Some(6), Some(6)));
    assert_eq!(v["meta"]["verify"], true);
    // 整体只写一条历史
    let h = run_cli(&["history", "--json"], dir.path());
    let runs: serde_json::Value = serde_json::from_slice(&h.stdout).unwrap();
    assert_eq!(runs.as_array().unwrap().len(), 1, "{runs}");
}

/// 导出结果：缺省 = 全部用例的最新结果；也可按运行 ID 导出；没有结果、格式不对都是 exit 2
#[test]
fn export_writes_latest_results_or_one_run() {
    let dir = TempDir::new().unwrap();
    write_verify_project(dir.path(), spawn_echo());
    let empty = run_cli(&["export"], dir.path());
    assert_eq!(empty.status.code(), Some(2), "{}", String::from_utf8_lossy(&empty.stderr));
    assert!(String::from_utf8_lossy(&empty.stderr).contains("还没有可导出的运行结果"));

    assert_eq!(run_cli(&["run", "collections"], dir.path()).status.code(), Some(0));
    let out_dir = dir.path().join("out");
    std::fs::create_dir_all(&out_dir).unwrap();
    let html = run_cli(&["export", "-o", out_dir.to_str().unwrap()], dir.path());
    assert_eq!(html.status.code(), Some(0), "{}", String::from_utf8_lossy(&html.stderr));
    let files: Vec<_> = std::fs::read_dir(&out_dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
    assert_eq!(files.len(), 1, "{files:?}");
    assert!(files[0].starts_with("验收报告-") && files[0].ends_with(".html"), "{files:?}");
    assert!(std::fs::read_to_string(out_dir.join(&files[0])).unwrap().contains("requestbaby 验收报告"));

    let json_path = out_dir.join("r.json");
    assert_eq!(run_cli(&["export", "-f", "json", "-o", json_path.to_str().unwrap()], dir.path()).status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&json_path).unwrap()).unwrap();
    assert_eq!(v["failed"], 0);
    assert!(v["total"].as_u64().unwrap() >= 1);
    assert_eq!(v["meta"]["source"], "history");

    let h = run_cli(&["history", "--json"], dir.path());
    let runs: serde_json::Value = serde_json::from_slice(&h.stdout).unwrap();
    let id = runs[0]["meta"]["id"].as_str().unwrap().to_string();
    let one = out_dir.join("one.json");
    assert_eq!(run_cli(&["export", &id, "-f", "json", "-o", one.to_str().unwrap()], dir.path()).status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&one).unwrap()).unwrap();
    assert_eq!(v["meta"]["id"], id.as_str());

    // 只导出一个集合：可以省略 collections/；该集合之外的用例不在里面
    let site = out_dir.join("site.json");
    assert_eq!(run_cli(&["export", "-c", "site", "-f", "json", "-o", site.to_str().unwrap()], dir.path()).status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&site).unwrap()).unwrap();
    assert!(v["results"].as_array().unwrap().iter().all(|r| r["file"].as_str().unwrap().starts_with("collections/site/")), "{v}");
    assert!(v["meta"]["command"].as_str().unwrap().ends_with("-c collections/site"));
    // 多个集合：-c 可重复；其中有没结果的集合要明说，不静默漏掉
    let two = run_cli(&["export", "-c", "site", "-c", "collections/none", "-f", "json", "-o", site.to_str().unwrap()], dir.path());
    assert_eq!(two.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&two.stderr).contains("collections/none"), "{}", String::from_utf8_lossy(&two.stderr));
    let nope = run_cli(&["export", "-c", "collections/none"], dir.path());
    assert_eq!(nope.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&nope.stderr).contains("没有运行结果"));
    assert_eq!(run_cli(&["export", &id, "-c", "site"], dir.path()).status.code(), Some(2));

    assert_eq!(run_cli(&["export", "-f", "pdf"], dir.path()).status.code(), Some(2));
    assert_eq!(run_cli(&["export", "no-such-run"], dir.path()).status.code(), Some(2));
}

#[test]
fn verify_blocked_by_validation_exits_2_with_reason() {
    let dir = TempDir::new().unwrap();
    write_verify_project(dir.path(), spawn_echo());
    let out = run_cli(&["verify", "-e", "staging", "--report", "json"], dir.path());
    assert_eq!(out.status.code(), Some(2));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["blocked"].as_str().unwrap().contains("校验"), "{v}");
    assert!(v["issues"].as_array().unwrap().iter().any(|i| i["rule"] == "env-missing"), "{v}");
    let statuses: Vec<&str> = v["steps"].as_array().unwrap().iter().map(|s| s["status"].as_str().unwrap()).collect();
    assert_eq!(statuses[0], "failed");
    assert!(statuses[1..].iter().all(|s| *s == "notRun"), "{statuses:?}");
}

fn write_gate_project(root: &std::path::Path, port: u16, workflows: serde_json::Value) {
    std::fs::create_dir(root.join("envs")).unwrap();
    std::fs::write(root.join("requestbaby.json"), serde_json::json!({"defaultEnv":"test", "workflows":workflows}).to_string()).unwrap();
    for env in ["test", "local"] {
        std::fs::write(root.join(format!("envs/{env}.json")), format!(r#"{{"vars":{{"baseUrl":"http://127.0.0.1:{port}"}}}}"#)).unwrap();
    }
}

fn gate_case(root: &std::path::Path, file: &str, body: serde_json::Value) {
    let path = root.join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body.to_string()).unwrap();
}

fn gate_result(root: &std::path::Path, extra: &[&str]) -> (i32, serde_json::Value) {
    let mut args = vec!["verify", "--report", "json"];
    if !extra.contains(&"--plan") {
        args.push("--no-history");
    }
    args.extend(extra);
    let out = run_cli(&args, root);
    let report = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)));
    (out.status.code().unwrap(), report)
}

#[test]
fn verify_without_any_execution_is_incomplete() {
    let dir = TempDir::new().unwrap();
    for (root, workflows) in [
        (dir.path().join("empty"), serde_json::json!({})),
        (dir.path().join("pinned"), serde_json::json!({"onlyLocal":{"env":"local","steps":[{"run":"fixtures/only.json"}]}})),
    ] {
        std::fs::create_dir(&root).unwrap();
        write_gate_project(&root, 9, workflows.clone());
        if workflows.get("onlyLocal").is_some() {
            gate_case(&root, "fixtures/only.json", serde_json::json!({"request":{"method":"GET","url":"{{baseUrl}}/ok"},"expect":{"status":200}}));
        }
        let (code, report) = gate_result(&root, &[]);
        assert_eq!((code, report["total"].as_u64(), report["coverage"]["checked"].as_u64()), (2, Some(0), Some(0)), "{report}");
    }
    let root = dir.path().join("empty");
    gate_case(&root, "collections/collection.json", serde_json::json!({"setup":"throw new Error('skipped collection must not be set up')"}));
    gate_case(&root, "collections/skipped.json", serde_json::json!({"skip":true,"request":{"method":"GET","url":"{{baseUrl}}/ok"},"expect":{"status":200}}));
    let (code, report) = gate_result(&root, &[]);
    assert_eq!(code, 2, "{report}");
    assert_eq!(report["coverage"]["checked"], 0);
    assert_eq!(report["coverage"]["unverified"][0]["reason"], "skipped");
}

#[test]
fn verify_explicit_required_cases_can_run_unasserted_setup() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    write_gate_project(root, spawn_echo(), serde_json::json!({"smoke":{"steps":[{"run":"collections"}]}}));
    gate_case(root, "collections/01-setup.json", serde_json::json!({"request":{"method":"GET","url":"{{baseUrl}}/setup"}}));
    let required = "collections/02-check.json";
    gate_case(root, required, serde_json::json!({"request":{"method":"GET","url":"{{baseUrl}}/check"},"expect":{"status":200}}));
    let (code, report) = gate_result(root, &["--workflow", "smoke", "--require-case", required]);
    assert_eq!(code, 0, "{report}");
    assert_eq!(report["total"], 2);
    assert_eq!(report["coverage"]["required"], 1);
    assert_eq!(report["coverage"]["checked"], 1);
}

#[test]
fn verify_reports_three_of_five_required_cases_checked() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    write_gate_project(root, spawn_echo(), serde_json::json!({"smoke":{"steps":[{"run":"collections"}]}}));
    let files: Vec<_> = (1..=5).map(|n| format!("collections/{n}.json")).collect();
    for (index, file) in files.iter().enumerate() {
        gate_case(root, file, serde_json::json!({"skip":index >= 3,"request":{"method":"GET","url":"{{baseUrl}}/ok"},"expect":{"status":200}}));
    }
    let mut args = vec!["--workflow", "smoke"];
    for file in &files {
        args.extend(["--require-case", file]);
    }
    let (code, report) = gate_result(root, &args);
    assert_eq!(code, 2, "{report}");
    let c = &report["coverage"];
    assert_eq!(c["required"], 5);
    assert_eq!(c["checked"], 3);
    let missing: Vec<_> = c["unverified"].as_array().unwrap().iter().map(|r| (r["file"].as_str().unwrap(), r["reason"].as_str().unwrap())).collect();
    assert_eq!(missing, [(files[3].as_str(), "skipped"), (files[4].as_str(), "skipped")]);
    assert!(report["blocked"].as_str().unwrap().contains("还有 2 个未验"));

    std::fs::remove_file(root.join(&files[4])).unwrap();
    let (code, report) = gate_result(root, &args);
    assert_eq!(code, 2, "{report}");
    assert_eq!(report["total"], 0);
    assert_eq!(report["coverage"]["unverified"][0]["reason"], "notSelected");
    assert_eq!(report["coverage"]["required"], 5);
}

#[test]
fn verify_distinguishes_a_missing_assertion_from_a_failed_assertion() {
    let dir = TempDir::new().unwrap();
    let port = spawn_server(|_| (500, "application/json".into(), vec![], br#"{"code":17}"#.to_vec()));
    write_gate_project(dir.path(), port, serde_json::json!({}));
    let file = "collections/01.json";
    let mut case = serde_json::json!({"request":{"method":"GET","url":"{{baseUrl}}/bad"}});
    gate_case(dir.path(), file, case.clone());
    let (code, report) = gate_result(dir.path(), &[]);
    assert_eq!(code, 2, "{report}");
    assert_eq!(report["results"][0]["status"], 500);
    assert_eq!(report["coverage"]["unverified"][0]["reason"], "noAssertion");
    case["expect"] = serde_json::json!({"status":200, "body":{"code":0}});
    gate_case(dir.path(), file, case);
    let (code, report) = gate_result(dir.path(), &[]);
    assert_eq!(code, 1, "{report}");
    assert_eq!(report["coverage"]["checked"], 1);
    assert_eq!(report["failed"], 1);
}

#[test]
fn verify_plan_never_sends_requests_and_combined_verify_blocks_duplicate_writes() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static REQUESTS: AtomicUsize = AtomicUsize::new(0);
    let port = spawn_server(|_| {
        REQUESTS.fetch_add(1, Ordering::SeqCst);
        (200, "application/json".into(), vec![], br#"{"code":0}"#.to_vec())
    });
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    write_gate_project(root, port, serde_json::json!({"smoke":{"steps":[{"run":"collections","tags":["smoke"],"as":"teacher"}]}}));
    gate_case(root, "collections/01.json", serde_json::json!({"name":"创建订单","tags":["smoke"],"request":{"method":"POST","url":"{{baseUrl}}/new"},"expect":{"body":{"code":0}}}));
    gate_case(root, "collections/02.json", serde_json::json!({"name":"其他用例","request":{"method":"GET","url":"{{baseUrl}}/other"},"expect":{"status":200}}));

    let (code, plan) = gate_result(root, &["--plan"]);
    assert_eq!(code, 2, "{plan}");
    assert!(plan["blocked"].as_str().unwrap().contains("多个阶段"));
    assert_eq!(plan["stages"][2]["cases"].as_array().unwrap().len(), 1);
    assert_eq!(plan["stages"][2]["cases"][0]["user"], "teacher");
    assert_eq!(REQUESTS.load(Ordering::SeqCst), 0);
    assert!(!root.join(".requestbaby").exists());

    let (code, report) = gate_result(root, &[]);
    assert_eq!(code, 2, "{report}");
    assert_eq!(REQUESTS.load(Ordering::SeqCst), 0);

    let (code, plan) = gate_result(root, &["--workflow", "smoke", "--plan"]);
    assert_eq!(code, 0, "{plan}");
    assert_eq!(plan["stages"].as_array().unwrap().len(), 2);
    assert_eq!(plan["stages"][1]["cases"][0]["method"], "POST");
    assert_eq!(REQUESTS.load(Ordering::SeqCst), 0);

    let (code, report) = gate_result(root, &["--workflow", "smoke"]);
    assert_eq!(code, 0, "{report}");
    assert_eq!(report["coverage"]["checked"], 1);
    assert_eq!(REQUESTS.load(Ordering::SeqCst), 1);
}

#[test]
fn verify_reports_image_count_and_checks_the_captured_image_response() {
    let port = spawn_server(|raw| {
        let line = String::from_utf8_lossy(raw);
        if line.starts_with("GET /images ") {
            (200, "application/json".into(), vec![], br#"{"code":0,"data":{"pages":[{"url":"/pic"},{"url":"/pic2"}]}}"#.to_vec())
        } else {
            (200, "image/png".into(), vec![], b"\x89PNG\r\n\x1a\n".to_vec())
        }
    });
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    write_gate_project(root, port, serde_json::json!({"report":{"steps":[{"run":"collections"}]}}));
    let listing = "collections/01-images.json";
    let first = serde_json::json!({
        "request":{"method":"GET","url":"{{baseUrl}}/images"},
        "expect":{"status":200,"body":{"code":0,"data.pages":{"len":2}}},
        "capture":{"image":"data.pages[0].url"}
    });
    gate_case(root, listing, first.clone());
    gate_case(root, "collections/02-download.json", serde_json::json!({
        "request":{"method":"GET","url":"{{baseUrl}}{{image}}"},
        "expect":{"status":200,"headers":{"content-type":{"startsWith":"image/"}}}
    }));
    let (code, report) = gate_result(root, &["--workflow", "report"]);
    assert_eq!(code, 0, "{report}");
    assert_eq!(report["coverage"]["checked"], 2);

    let mut changed = first;
    changed["expect"]["body"]["data.pages"]["len"] = 3.into();
    gate_case(root, listing, changed);
    let (code, report) = gate_result(root, &["--workflow", "report"]);
    assert_eq!(code, 1, "{report}");
    assert_eq!(report["coverage"]["checked"], 1);
    assert_eq!(report["coverage"]["unverified"][0]["reason"], "notRun");
}

/// 并发跑：结果按文件顺序输出；HEAD 不等正文（服务按规范只回头）
#[test]
fn parallel_results_keep_file_order_and_head_requests_finish() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    run_cli(&["init", "."], d);
    std::fs::remove_dir_all(d.join("collections")).unwrap();
    std::fs::create_dir_all(d.join("collections/p")).unwrap();
    let port = spawn_echo();
    run_cli(&["vars", "set", "baseUrl", &format!("http://127.0.0.1:{port}"), "-e", "dev"], d);
    for i in 1..=12 {
        let method = if i % 4 == 0 { "HEAD" } else { "GET" };
        std::fs::write(
            d.join(format!("collections/p/{i:02}-c.json")),
            format!(r#"{{"name":"c{i}","request":{{"method":"{method}","url":"{{{{baseUrl}}}}/n/{i}","timeoutMs":3000}},"expect":{{"status":200}}}}"#),
        )
        .unwrap();
    }
    let out = run_cli(&["run", "--parallel", "4", "--report", "json"], d);
    let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(0), "{r:#}");
    let names: Vec<&str> = r["results"].as_array().unwrap().iter().map(|x| x["name"].as_str().unwrap()).collect();
    let want: Vec<String> = (1..=12).map(|i| format!("c{i}")).collect();
    assert_eq!(names, want);
}

/// 测试内回显：把收到的原始 HTTP 请求原样作为响应体返回（并下发一个 Cookie），用于检查线上实际发出的字节
fn spawn_echo() -> u16 {
    spawn_server(|raw| (200, "text/plain".into(), vec![("Set-Cookie".into(), "sid=s-42; Path=/".into())], raw.to_vec()))
}

/// 测试内 JSON 接口：/login 下发 token 与 X-Trace-Id；/me 回显 Authorization（检查捕获的变量是否被带上）
fn spawn_json_api() -> u16 {
    spawn_server(|raw| {
        let text = String::from_utf8_lossy(raw).to_string();
        let first = text.lines().next().unwrap_or_default().to_string();
        let auth = text.lines().find_map(|l| l.strip_prefix("Authorization: ").or_else(|| l.strip_prefix("authorization: "))).unwrap_or("").trim().to_string();
        let (status, body, headers) = if first.starts_with("GET /login") {
            (200, serde_json::json!({"code": 0, "data": {"token": "tk-1", "uid": 42}}), vec![("X-Trace-Id".to_string(), "tr-7".to_string())])
        } else if first.starts_with("GET /me") {
            let body = serde_json::json!({"code": 0, "data": {"uid": 42, "auth": auth, "items": [{"id": 1, "price": 9.5}, {"id": 2, "price": 3}]}});
            (200, body, vec![])
        } else {
            (404, serde_json::json!({"code": 404}), vec![])
        };
        (status, "application/json".into(), headers, body.to_string().into_bytes())
    })
}

/// 测试响应：(状态码, Content-Type, 额外响应头, 响应体)
type TestResponse = (u16, String, Vec<(String, String)>, Vec<u8>);

/// 测试内 HTTP 响应器：读完一个请求（头 + Content-Length 体），交给 handler 生成响应
fn spawn_server(handler: fn(&[u8]) -> TestResponse) -> u16 {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for mut s in listener.incoming().flatten() {
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let n = s.read(&mut chunk).unwrap_or(0);
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&buf[..end]).to_lowercase();
                    let len = head
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length:"))
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if buf.len() >= end + 4 + len {
                        break;
                    }
                }
            }
            let (status, ctype, headers, body) = handler(&buf);
            let mut head = format!("HTTP/1.1 {status} X\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
            for (k, v) in headers {
                head.push_str(&format!("{k}: {v}\r\n"));
            }
            head.push_str("\r\n");
            let _ = s.write_all(head.as_bytes());
            // HEAD 的响应只有头（Content-Length 仍是正文长度），与真实服务一致
            if !buf.starts_with(b"HEAD ") {
                let _ = s.write_all(&body);
            }
        }
    });
    port
}

/// 表单 / multipart 文件上传 / query 参数 / 停用请求头 / Cookie 自动回传：检查实际发出的请求
#[test]
fn form_multipart_query_and_cookies_go_on_the_wire() {
    let port = spawn_echo();
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    std::fs::create_dir_all(d.join("envs")).unwrap();
    std::fs::create_dir_all(d.join("collections/fixtures")).unwrap();
    std::fs::write(d.join("requestbaby.json"), r#"{"version":1,"defaultEnv":"t"}"#).unwrap();
    std::fs::write(d.join("envs/t.json"), format!(r#"{{"vars":{{"baseUrl":"http://127.0.0.1:{port}","who":"张 三"}}}}"#)).unwrap();
    std::fs::write(d.join("collections/fixtures/report.csv"), "id,score\n1,98\n").unwrap();
    std::fs::write(
        d.join("collections/01-form.json"),
        r#"{"request":{"method":"POST","url":"{{baseUrl}}/login?from=web",
             "query":{"lang":"zh cn","debug":{"value":"1","disabled":true}},
             "headers":{"X-On":"yes","X-Off":{"value":"no","disabled":true}},
             "body":{"form":{"user":"{{who}}","pwd":"a&b"}}},
           "tests":[
             "res.text().startsWith('POST /login?from=web&lang=zh%20cn HTTP/1.1')",
             "res.text().includes('Content-Type: application/x-www-form-urlencoded')",
             "res.text().endsWith('pwd=a%26b&user=%E5%BC%A0%20%E4%B8%89')",
             "res.text().includes('X-On: yes') && !res.text().includes('X-Off')"
           ]}"#,
    )
    .unwrap();
    std::fs::write(
        d.join("collections/02-upload.json"),
        r#"{"request":{"method":"PUT","url":"{{baseUrl}}/upload",
             "body":{"multipart":{"title":"月报","file":{"file":"fixtures/report.csv","type":"text/csv"}}}},
           "tests":[
             "res.text().startsWith('PUT /upload ')",
             "res.text().includes('Content-Type: multipart/form-data; boundary=')",
             "res.text().includes('name=\"file\"; filename=\"report.csv\"')",
             "res.text().includes('Content-Type: text/csv')",
             "res.text().includes('id,score\\n1,98')",
             "res.text().includes('月报')",
             "res.text().includes('Cookie: sid=s-42')"
           ]}"#,
    )
    .unwrap();
    let out = run_cli(&["run", "collections"], d);
    let stdout = strip_ansi(&String::from_utf8_lossy(&out.stdout));
    assert!(out.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));

    // dry-run 的 curl 命令可直接粘贴执行：含 -F 与 --data-raw，& 已加引号
    let out = run_cli(&["run", "collections", "--dry-run"], d);
    let stdout = strip_ansi(&String::from_utf8_lossy(&out.stdout));
    assert!(stdout.contains("--data-raw 'pwd=a%26b&user="), "{stdout}");
    assert!(stdout.contains("-F 'file=@"), "{stdout}");
}

/// 重跑失败 + 与历史对比：失败 → --failed 只跑失败的及其前置（捕获 token 的登录）→ 修复后标记「已修复」
#[test]
fn rerun_failed_with_prerequisites_and_compare() {
    let port = spawn_echo();
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    std::fs::create_dir_all(d.join("envs")).unwrap();
    std::fs::create_dir_all(d.join("collections")).unwrap();
    std::fs::write(d.join("requestbaby.json"), r#"{"version":1,"defaultEnv":"t"}"#).unwrap();
    std::fs::write(d.join("envs/t.json"), format!(r#"{{"vars":{{"baseUrl":"http://127.0.0.1:{port}","flag":"off"}}}}"#)).unwrap();
    std::fs::write(
        d.join("collections/01-login.json"),
        r#"{"request":{"method":"POST","url":"{{baseUrl}}/login"},"post":"vars.token = 'tk-1'","tests":["res.status === 200"]}"#,
    )
    .unwrap();
    std::fs::write(
        d.join("collections/02-me.json"),
        r#"{"request":{"method":"GET","url":"{{baseUrl}}/me","headers":{"Authorization":"Bearer {{token}}","X-Flag":"{{flag}}"}},
           "tests":["res.text().includes('Bearer tk-1')","res.text().includes('X-Flag: on')"]}"#,
    )
    .unwrap();
    std::fs::write(d.join("collections/03-health.json"), r#"{"request":{"method":"GET","url":"{{baseUrl}}/health"},"tests":["res.status === 200"]}"#).unwrap();
    let json = |args: &[&str]| -> serde_json::Value {
        let out = run_cli(args, d);
        serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stdout)))
    };

    let r1 = json(&["run", "--report", "json"]);
    assert_eq!(r1["failed"], 1);
    assert!(r1.get("compare").is_none(), "首次运行没有对比基准");

    // --failed：只跑 02，并自动带上捕获 token 的 01；03 不跑
    let r2 = json(&["run", "--failed", "--report", "json"]);
    let files: Vec<&str> = r2["results"].as_array().unwrap().iter().map(|r| r["file"].as_str().unwrap()).collect();
    assert_eq!(files, ["collections/01-login.json", "collections/02-me.json"]);
    assert_eq!(r2["compare"]["stillFailing"][0]["file"], "collections/02-me.json");
    assert_eq!(r2["meta"]["command"], "requestbaby run collections --failed");

    assert!(run_cli(&["vars", "set", "flag", "on", "-e", "t"], d).status.success());
    let out = run_cli(&["run"], d);
    let stdout = strip_ansi(&String::from_utf8_lossy(&out.stdout));
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains("对比上次：新失败 0  已修复 1  仍失败 0"), "{stdout}");

    // 全部通过后 --failed 无事可做：说明一下，退出码 0
    let out = run_cli(&["run", "--failed"], d);
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("没有最近失败的用例"));
}

/// 固定环境的流程：verify -e test 跳过固定在 local 的 fixture 流程（不会把 fixture 打到 test）；
/// 在 local 运行它正常执行；显式 -e test 运行它被拒绝
#[test]
fn pinned_workflow_env_is_skipped_by_verify_in_other_env() {
    let echo = spawn_echo();
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    // 两个环境都指向回显，只是 baseUrl 写法不同，fixture 的 setup 用它区分环境
    let local = format!("http://localhost:{echo}");
    std::fs::create_dir_all(d.join("envs")).unwrap();
    std::fs::create_dir_all(d.join("collections")).unwrap();
    std::fs::create_dir_all(d.join("fixtures/01-local")).unwrap();
    std::fs::write(
        d.join("requestbaby.json"),
        r#"{"version":1,"defaultEnv":"test","workflows":{"local-fixture":{"env":"local","steps":[{"run":"fixtures/01-local"}]}}}"#,
    )
    .unwrap();
    std::fs::write(d.join("envs/test.json"), format!(r#"{{"vars":{{"baseUrl":"http://127.0.0.1:{echo}"}}}}"#)).unwrap();
    std::fs::write(d.join("envs/local.json"), format!(r#"{{"vars":{{"baseUrl":"{local}"}}}}"#)).unwrap();
    std::fs::write(d.join("collections/01-ping.json"), r#"{"request":{"method":"GET","url":"{{baseUrl}}/ping"},"expectStatus":200}"#).unwrap();
    std::fs::write(
        d.join("fixtures/01-local/collection.json"),
        format!(r#"{{"setup":"if (env.baseUrl !== '{local}') throw new Error('本地 fixture 只能使用 local 环境')"}}"#),
    )
    .unwrap();
    std::fs::write(d.join("fixtures/01-local/01-health.json"), r#"{"request":{"method":"GET","url":"{{baseUrl}}/health"},"expectStatus":200}"#)
        .unwrap();

    let out = run_cli(&["verify", "--report", "json"], d);
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stdout)));
    assert_eq!(out.status.code(), Some(0), "{report:#}");
    let stages: Vec<(&str, &str)> =
        report["steps"].as_array().unwrap().iter().map(|s| (s["label"].as_str().unwrap(), s["status"].as_str().unwrap())).collect();
    assert!(stages.contains(&("流程 local-fixture（local）", "notRun")), "{stages:?}");
    assert_eq!(report["passed"], 1);
    assert!(String::from_utf8_lossy(&run_cli(&["verify"], d).stdout).contains("固定在环境 local，本次跳过"));

    let out = run_cli(&["verify", "-e", "local", "--report", "json"], d);
    let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(0), "{r:#}");
    assert_eq!(r["passed"], 2);

    let out = run_cli(&["workflow", "local-fixture", "-e", "test"], d);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("固定在环境 local"), "{}", String::from_utf8_lossy(&out.stderr));

    let out = run_cli(&["workflow", "local-fixture", "--report", "json"], d);
    let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(0), "{r:#}");
    assert_eq!(r["meta"]["env"], "local");
    assert_eq!(r["meta"]["command"], "requestbaby workflow local-fixture");

    // 集合 setup 失败：结果行指向 collection.json，名称写明是集合 setup
    std::fs::write(
        d.join("requestbaby.json"),
        r#"{"version":1,"defaultEnv":"test","workflows":{"local-fixture":{"steps":[{"run":"fixtures/01-local"}]}}}"#,
    )
    .unwrap();
    let out = run_cli(&["workflow", "local-fixture", "--report", "json"], d);
    let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(r["results"][0]["file"], "fixtures/01-local/collection.json");
    assert_eq!(r["results"][0]["name"], "集合 setup：01-local");
    assert_eq!(r["results"][0]["method"], "setup");
}

/// 环境 / 用例 / 集合的增删改查：CLI 与桌面端同一实现（core::envs / core::scaffold）
#[test]
fn env_case_collection_crud() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    assert!(run_cli(&["init"], d).status.success());
    let ok = |args: &[&str]| {
        let out = run_cli(args, d);
        assert_eq!(out.status.code(), Some(0), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    };
    ok(&["env", "cp", "dev", "staging"]);
    ok(&["workflow", "new", "smoke", "--step", "collections/demo", "-e", "staging"]);
    ok(&["env", "rename", "staging", "qa"]);
    let project: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("requestbaby.json")).unwrap()).unwrap();
    assert_eq!(project["workflows"]["smoke"]["env"], "qa");
    let out = run_cli(&["env", "rm", "qa"], d);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("流程 smoke"));
    ok(&["env", "default", "prod"]);
    let out = run_cli(&["env", "list", "--json"], d);
    let list: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let defaults: Vec<&str> = list.as_array().unwrap().iter().filter(|e| e["isDefault"] == true).map(|e| e["name"].as_str().unwrap()).collect();
    assert_eq!(defaults, ["prod"]);

    ok(&["case", "cp", "collections/demo/01-health.json", "09-health-copy"]);
    assert!(d.join("collections/demo/09-health-copy.json").exists());
    ok(&["collection", "cp", "collections/demo", "demo2"]);
    ok(&["collection", "rename", "collections/demo2", "demo3"]);
    assert!(d.join("collections/demo3/09-health-copy.json").exists());
    assert_eq!(run_cli(&["case", "rm", "collections/demo3"], d).status.code(), Some(2), "case rm 只接受用例文件");
}

/// 声明式断言与捕获：登录捕获 token / 数字 uid / 响应头 → 下一个用例带上并用 expect 校验；失败写明字段、期望与实际
#[test]
fn expect_and_capture_end_to_end() {
    let port = spawn_json_api();
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    std::fs::create_dir_all(d.join("envs")).unwrap();
    std::fs::create_dir_all(d.join("collections")).unwrap();
    std::fs::write(d.join("requestbaby.json"), r#"{"version":1,"defaultEnv":"dev"}"#).unwrap();
    std::fs::write(d.join("envs/dev.json"), format!(r#"{{"vars":{{"baseUrl":"http://127.0.0.1:{port}"}}}}"#)).unwrap();
    std::fs::write(
        d.join("collections/01-login.json"),
        r#"{"request":{"method":"GET","url":"{{baseUrl}}/login"},
            "expect":{"status":200,"headers":{"x-trace-id":{"startsWith":"tr-"}},"body":{"code":0,"data.uid":{"type":"integer","gt":0}}},
            "capture":{"token":"data.token","uid":"data.uid","trace":"header:X-Trace-Id"}}"#,
    )
    .unwrap();
    std::fs::write(
        d.join("collections/02-me.json"),
        r#"{"request":{"method":"GET","url":"{{baseUrl}}/me","headers":{"Authorization":"Bearer {{token}}"}},
            "expect":{"status":[200,201],"body":{"data.auth":"Bearer {{token}}","data.uid":"{{uid}}","data.items":{"len":2},"data.items[*].price":{"gt":0}}},
            "tests":["vars.trace === 'tr-7'"]}"#,
    )
    .unwrap();
    std::fs::write(
        d.join("collections/03-wrong.json"),
        r#"{"request":{"method":"GET","url":"{{baseUrl}}/me"},"expect":{"body":{"code":1,"data.items[*].price":{"gt":5}}}}"#,
    )
    .unwrap();

    let out = run_cli(&["validate", "--report", "json"], d);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stdout));

    let out = run_cli(&["run", "--report", "json"], d);
    let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(1), "{r:#}");
    assert_eq!(r["passed"], 2, "{r:#}");
    let me = &r["results"][1];
    let exprs: Vec<&str> = me["tests"].as_array().unwrap().iter().map(|t| t["expr"].as_str().unwrap()).collect();
    assert_eq!(exprs[0], "status ∈ [200, 201]");
    // 报告里展示代入变量后的期望值；数字变量保持数字（42 而不是 "42"）
    assert_eq!(&exprs[1..3], ["body.data.auth = \"Bearer tk-1\"", "body.data.uid = 42"]);
    assert!(me["tests"].as_array().unwrap().iter().all(|t| t["pass"] == true), "{me:#}");
    let wrong: Vec<String> = r["results"][2]["tests"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["pass"] == false)
        .map(|t| t["detail"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(wrong, ["期望 1，实际 0", "data.items[1].price：期望 > 5，实际 3（共 2 个）"]);

    // 并发运行：捕获 token 的登录必须先于引用它的用例（结果与串行一致）
    let out = run_cli(&["run", "--parallel", "4", "--report", "json"], d);
    let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!((r["passed"].as_i64(), r["failed"].as_i64()), (Some(2), Some(1)), "{r:#}");

    // 失败摘要（复制给 AI / 同事）里直接写明原因
    let out = run_cli(&["run", "collections/03-wrong.json"], d);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("body.code = 1") && text.contains("期望 1，实际 0"), "{text}");

    // run --failed：只重跑失败的，并带上 capture 了它所需变量的前置用例
    std::fs::write(
        d.join("collections/02-me.json"),
        r#"{"request":{"method":"GET","url":"{{baseUrl}}/me","headers":{"Authorization":"Bearer {{token}}"}},"expect":{"body":{"data.auth":"Bearer {{token}}","code":1}}}"#,
    )
    .unwrap();
    run_cli(&["run"], d);
    let out = run_cli(&["run", "--failed", "--report", "json"], d);
    let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let files: Vec<&str> = r["results"].as_array().unwrap().iter().map(|x| x["file"].as_str().unwrap()).collect();
    assert_eq!(files, ["collections/01-login.json", "collections/02-me.json", "collections/03-wrong.json"], "{r:#}");
    let me_tests = &r["results"][1]["tests"];
    assert!(me_tests.as_array().unwrap().iter().any(|t| t["expr"] == "body.data.auth = \"Bearer tk-1\"" && t["pass"] == true), "{me_tests:#}");

    // 取不到值的 capture 让用例失败并说明缺什么
    std::fs::write(
        d.join("collections/01-login.json"),
        r#"{"request":{"method":"GET","url":"{{baseUrl}}/login"},"capture":{"token":"data.accessToken"}}"#,
    )
    .unwrap();
    let out = run_cli(&["run", "collections/01-login.json", "--report", "json"], d);
    let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(r["results"][0]["tests"][0]["detail"], "响应中没有 data.accessToken");

    // 写错的运算符 / 捕获路径：validate 直接报出（exit 2）
    std::fs::write(
        d.join("collections/04-bad.json"),
        r#"{"request":{"method":"GET","url":"{{baseUrl}}/me"},"expect":{"body":{"code":{"gt":0,"lenn":1},"data.items[x]":1}},"capture":{"a":"data.items[*].id"}}"#,
    )
    .unwrap();
    let out = run_cli(&["validate", "collections/04-bad.json", "--report", "json"], d);
    assert_eq!(out.status.code(), Some(2));
    let issues: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let rules: Vec<&str> = issues["issues"].as_array().unwrap().iter().map(|i| i["rule"].as_str().unwrap()).collect();
    assert!(rules.contains(&"expect-matcher") && rules.contains(&"expect-path") && rules.contains(&"capture-source"), "{issues:#}");
}

/// 写法建议：validate 给出可改写提示 → fmt --fix 改写为 expect / capture / {{baseUrl}} → 结果不变、不再有可改写项
#[test]
fn fmt_fix_upgrades_legacy_cases_without_changing_results() {
    let port = spawn_json_api();
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    std::fs::create_dir_all(d.join("envs")).unwrap();
    std::fs::create_dir_all(d.join("collections/fx")).unwrap();
    let base = format!("http://127.0.0.1:{port}");
    std::fs::write(
        d.join("requestbaby.json"),
        r#"{"version":1,"defaultEnv":"dev","workflows":{"fx":{"env":"dev","steps":[{"run":"collections/fx"}]}}}"#,
    )
    .unwrap();
    std::fs::write(d.join("envs/dev.json"), format!(r#"{{"vars":{{"baseUrl":"{base}"}}}}"#)).unwrap();
    std::fs::write(
        d.join("collections/fx/collection.json"),
        format!(r#"{{"setup":"if(env.baseUrl!=='{base}') throw new Error('只能在 dev 跑');"}}"#),
    )
    .unwrap();
    std::fs::write(
        d.join("collections/fx/01-login.json"),
        format!(
            r#"{{"expectStatus":200,"request":{{"method":"GET","url":"{base}/login"}},
                "post":"vars.token = res.json('data.token'); vars.uid = data.data.uid",
                "tests":["data.code === 0 && typeof data.data.token === 'string'","res.headers['x-trace-id'].startsWith('tr-')"]}}"#
        ),
    )
    .unwrap();
    std::fs::write(
        d.join("collections/fx/02-me.json"),
        r#"{"request":{"method":"GET","url":"{{baseUrl}}/me","headers":{"Authorization":"Bearer {{token}}"}},
            "tests":["res.status === 200","data.data.uid === vars.uid","data.data.auth === 'Bearer ' + vars.token","data.data.items.length === 2","data.data.items.every(x => x.price > 0)"]}"#,
    )
    .unwrap();

    let summary = |d: &std::path::Path| -> (i64, i64) {
        let out = run_cli(&["run", "collections", "--report", "json"], d);
        let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        (r["passed"].as_i64().unwrap(), r["failed"].as_i64().unwrap())
    };
    assert_eq!(summary(d), (2, 0));

    let out = run_cli(&["validate", "--report", "json"], d);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["errors"], 0, "{v:#}");
    let fixable: Vec<&str> = v["issues"].as_array().unwrap().iter().filter(|i| i["fixable"] == true).map(|i| i["rule"].as_str().unwrap()).collect();
    for rule in ["env-guard-script", "legacy-expect-status", "prefer-expect", "prefer-capture", "hardcoded-base-url"] {
        assert!(fixable.contains(&rule), "{rule} 应可改写：{v:#}");
    }

    // --check 只报告，不写文件
    let before = std::fs::read_to_string(d.join("collections/fx/01-login.json")).unwrap();
    let out = run_cli(&["fmt", "--fix", "--check"], d);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stdout).contains("可改写"));
    assert_eq!(std::fs::read_to_string(d.join("collections/fx/01-login.json")).unwrap(), before);

    let out = run_cli(&["fmt", "--fix"], d);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stdout));
    let login: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("collections/fx/01-login.json")).unwrap()).unwrap();
    assert_eq!(
        login,
        serde_json::json!({
            "request": {"method": "GET", "url": "{{baseUrl}}/login"},
            "expect": {"status": 200, "headers": {"x-trace-id": {"startsWith": "tr-"}}, "body": {"code": 0, "data.token": {"type": "string"}}},
            "capture": {"token": "data.token", "uid": "data.uid"}
        })
    );
    let me: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("collections/fx/02-me.json")).unwrap()).unwrap();
    assert_eq!(me["expect"]["body"]["data.auth"], "Bearer {{token}}");
    assert_eq!(me["tests"], serde_json::json!(["data.data.items.every(x => x.price > 0)"]), "需要计算的保留为 JS");
    let col: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("collections/fx/collection.json")).unwrap()).unwrap();
    assert!(col.get("setup").is_none(), "流程已固定环境，守卫脚本被删除");

    // 结果不变；再检查没有可改写项
    assert_eq!(summary(d), (2, 0));
    let out = run_cli(&["fmt", "--fix", "--check"], d);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stdout));
    let out = run_cli(&["validate", "--report", "json"], d);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["issues"].as_array().unwrap().iter().all(|i| i["fixable"] != true), "{v:#}");
}

/// gRPC / WebSocket 用例：配置了 .proto 时 validate 不连服务就核对方法与消息；步骤写法错误、协议键冲突都有明确提示；
/// dry-run 输出等价的 grpcurl / websocat 命令；fmt 按规范顺序写回
#[test]
fn grpc_and_ws_cases_validate_dry_run_and_fmt() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    std::fs::create_dir_all(d.join("collections/rpc")).unwrap();
    std::fs::create_dir_all(d.join("protos/shop")).unwrap();
    std::fs::create_dir_all(d.join("envs")).unwrap();
    std::fs::write(d.join("requestbaby.json"), r#"{"version":1,"defaultEnv":"dev","grpc":{"protos":["protos"]}}"#).unwrap();
    std::fs::write(d.join("envs/dev.json"), r#"{"vars":{"grpcTarget":"127.0.0.1:9","wsBase":"ws://127.0.0.1:9","token":"t"}}"#).unwrap();
    std::fs::write(
        d.join("protos/shop/order.proto"),
        "syntax = \"proto3\";\npackage shop.v1;\nmessage Req { int64 id = 1; }\nmessage Order { int64 id = 1; }\nservice OrderService {\n  rpc GetOrder(Req) returns (Order);\n  rpc Watch(Req) returns (stream Order);\n}\n",
    )
    .unwrap();
    let write = |name: &str, body: &str| std::fs::write(d.join("collections/rpc").join(name), body).unwrap();
    write("01-ok.json", r#"{"expect":{"status":"OK","body":{"id":1}},"grpc":{"method":"shop.v1.OrderService/GetOrder","target":"{{grpcTarget}}","message":{"id":1},"metadata":{"authorization":"Bearer {{token}}"}},"name":"查询"}"#);
    write(
        "02-ws.json",
        r#"{"name":"对话","ws":{"url":"{{wsBase}}/chat","steps":[{"send":{"type":"join"}},{"capture":{"sid":"sid"},"receive":{"type":"joined"}},{"send":"hi {{sid}}"}]}}"#,
    );
    let out = run_cli(&["validate", "--report", "json"], d);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["issues"], serde_json::json!([]), "{v:#}");

    // dry-run：等价命令可复现
    let out = run_cli(&["run", "--dry-run", "--report", "json", "-v"], d);
    let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(out.status.success(), "{r:#}");
    let curl: Vec<&str> = r["results"].as_array().unwrap().iter().map(|x| x["curl"].as_str().unwrap()).collect();
    assert_eq!(
        curl[0],
        "grpcurl -plaintext -import-path protos -import-path . -proto shop/order.proto -max-time 30 -H 'authorization: Bearer t' -d '{\"id\":1}' 127.0.0.1:9 shop.v1.OrderService/GetOrder"
    );
    assert_eq!(curl[1], r#"printf '%s\n' '{"type":"join"}' 'hi {{sid}}' | websocat ws://127.0.0.1:9/chat"#);
    assert_eq!(r["results"][0]["protocol"], "grpc");
    assert_eq!(r["results"][0]["method"], "GRPC");

    // fmt：协议块按规范顺序写回
    run_cli(&["fmt"], d);
    let text = std::fs::read_to_string(d.join("collections/rpc/01-ok.json")).unwrap();
    let keys: Vec<&str> = ["\"name\"", "\"grpc\"", "\"target\"", "\"method\"", "\"message\"", "\"metadata\"", "\"expect\""].into_iter().collect();
    let pos: Vec<usize> = keys.iter().map(|k| text.find(k).unwrap()).collect();
    assert!(pos.windows(2).all(|w| w[0] < w[1]), "{text}");
    let text = std::fs::read_to_string(d.join("collections/rpc/02-ws.json")).unwrap();
    assert!(text.find("\"receive\"").unwrap() < text.find("\"capture\"").unwrap(), "{text}");

    // 写错的地方：方法不存在、消息字段写错、流式方法发多条、状态名不对、步骤多个动作、capture 写在 send 上、协议键冲突
    write("03-bad-method.json", r#"{"grpc":{"target":"{{grpcTarget}}","method":"shop.v1.OrderService/Get"}}"#);
    write("04-bad-field.json", r#"{"grpc":{"target":"{{grpcTarget}}","method":"shop.v1.OrderService/GetOrder","message":{"idd":1}}}"#);
    write("05-bad-many.json", r#"{"grpc":{"target":"{{grpcTarget}}","method":"shop.v1.OrderService/Watch","messages":[{},{}]}}"#);
    write("06-bad-status.json", r#"{"grpc":{"target":"{{grpcTarget}}","method":"shop.v1.OrderService/GetOrder"},"expect":{"status":200}}"#);
    write("07-bad-step.json", r#"{"ws":{"url":"{{wsBase}}/x","steps":[{"send":"a","wait":10},{"send":"b","capture":{"x":"y"}},{}]}}"#);
    write("08-both.json", r#"{"request":{"method":"GET","url":"{{wsBase}}"},"ws":{"url":"{{wsBase}}"}}"#);
    write("09-ws-url.json", r#"{"ws":{"url":"example.com/chat"}}"#);
    let out = run_cli(&["validate", "--report", "json"], d);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(2));
    let msgs: Vec<String> = v["issues"].as_array().unwrap().iter().map(|i| format!("{} {} {}", i["file"].as_str().unwrap().rsplit('/').next().unwrap(), i["rule"].as_str().unwrap(), i["message"].as_str().unwrap())).collect();
    let has = |file: &str, rule: &str, text: &str| assert!(msgs.iter().any(|m| m.starts_with(&format!("{file} {rule} ")) && m.contains(text)), "缺少 {file} {rule} {text}：{msgs:#?}");
    has("03-bad-method.json", "grpc-proto", "没有方法 Get（有：GetOrder、Watch）");
    has("04-bad-field.json", "grpc-proto", "可用字段：id");
    has("05-bad-many.json", "grpc-message", "服务端流方法，只能发一条消息");
    has("06-bad-status.json", "expect-status", "200 不是gRPC 状态");
    has("07-bad-step.json", "ws-step", "第 1 步写了多个动作（send、wait）");
    has("07-bad-step.json", "ws-step", "第 2 步：capture 只能写在 receive / waitFor 步骤上");
    has("07-bad-step.json", "ws-step", "第 3 步没有动作");
    has("08-both.json", "protocol", "request / grpc / ws / web / android / desktop 只能写一个");
    has("09-ws-url.json", "ws-url", "缺少协议");
}

/// new case --grpc：配置了 .proto 时不连服务也能填好请求消息骨架；--ws 生成可直接运行的连接用例
#[test]
fn new_grpc_and_ws_cases_from_cli() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    std::fs::create_dir_all(d.join("protos")).unwrap();
    std::fs::create_dir_all(d.join("collections/rpc")).unwrap();
    std::fs::write(d.join("requestbaby.json"), r#"{"version":1,"grpc":{"protos":["protos"]}}"#).unwrap();
    std::fs::write(d.join("protos/a.proto"), "syntax = \"proto3\";\npackage a;\nmessage Q { string name = 1; repeated int32 ids = 2; }\nservice S { rpc Find(Q) returns (Q); rpc Up(stream Q) returns (Q); }\n").unwrap();
    let out = run_cli(&["grpc", "list"], d);
    let text = strip_ansi(&String::from_utf8_lossy(&out.stdout));
    assert!(text.contains("a.S/Find") && text.contains("一元") && text.contains("a.S/Up") && text.contains("客户端流"), "{text}");
    let out = run_cli(&["new", "case", "collections/rpc", "查找", "--grpc", "a.S/Find"], d);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("collections/rpc/01-查找.json")).unwrap()).unwrap();
    assert_eq!(doc["grpc"]["target"], "{{grpcTarget}}");
    assert_eq!(doc["grpc"]["message"], serde_json::json!({"name": "", "ids": [0]}));
    assert_eq!(doc["expect"]["status"], "OK");
    let out = run_cli(&["new", "case", "collections/rpc", "连接", "--ws"], d);
    assert!(out.status.success());
    let doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("collections/rpc/02-连接.json")).unwrap()).unwrap();
    assert_eq!(doc["ws"]["url"], "{{wsBase}}/");
    assert_eq!(doc["expect"]["status"], 101);
}

/// 界面用例：脚手架、校验规则、dry-run 列出步骤（不需要浏览器 / 设备）
#[test]
fn web_and_android_cases_scaffold_validate_and_dry_run() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    std::fs::write(d.join("requestbaby.json"), r#"{"version":1,"defaultEnv":"dev"}"#).unwrap();
    std::fs::create_dir_all(d.join("envs")).unwrap();
    std::fs::write(d.join("envs/dev.json"), r#"{"vars":{"webBase":"http://127.0.0.1:9"}}"#).unwrap();
    assert!(run_cli(&["new", "case", "collections/ui", "首页", "--web"], d).status.success());
    assert!(run_cli(&["new", "case", "collections/ui", "启动", "--android", "com.example.shop"], d).status.success());
    let web: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("collections/ui/01-首页.json")).unwrap()).unwrap();
    assert_eq!((web["web"]["url"].as_str(), web["expect"]["status"].as_u64()), (Some("{{webBase}}/"), Some(200)));
    let app: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("collections/ui/02-启动.json")).unwrap()).unwrap();
    assert_eq!(app["android"]["app"], "com.example.shop");

    let steps = r#"[{"fill":{"label":"手机号"},"value":"138"},{"click":{"role":"button","name":"登录"}},{"expect":{"url":"/home"}}]"#;
    std::fs::write(d.join("collections/ui/03-登录.json"), format!(r#"{{"name":"登录","web":{{"url":"{{{{webBase}}}}/login","steps":{steps}}}}}"#)).unwrap();
    let out = run_cli(&["validate", "--report", "json"], d);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["issues"], serde_json::json!([]), "{v:#}");
    let out = run_cli(&["run", "--dry-run"], d);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{text}");
    let out = run_cli(&["run", "collections/ui/03-登录.json", "--dry-run", "-v"], d);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("#2 点击 按钮「登录」") && text.contains("#3 断言 地址包含「/home」"), "{text}");

    // 写错的地方：动作不适用于该端、定位写了两种、缺 value、步骤两个动作、Web 写 Android 定位、捕获来源
    let write = |name: &str, body: &str| std::fs::write(d.join("collections/ui").join(name), body).unwrap();
    write("04-bad.json", r#"{"web":{"url":"example.com","steps":[{"swipe":"up"},{"click":{"role":"button","text":"x"}},{"fill":{"label":"a"}},{"click":{"text":"a"},"hover":{"text":"b"}},{"click":{"id":"x"}},{"wait":1,"capture":{"a":"activity"}}]}}"#);
    write("05-bad.json", r#"{"android":{"app":"shop","steps":[{"select":{"id":"x"},"value":"1"},{"tap":{"css":".x"}},{"press":"Hyper"},{"expect":{"state":"hidden"}}]}}"#);
    let out = run_cli(&["validate", "--report", "json"], d);
    assert_eq!(out.status.code(), Some(2));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let msgs: Vec<String> = v["issues"].as_array().unwrap().iter().map(|i| format!("{} {} {}", i["file"].as_str().unwrap().rsplit('/').next().unwrap(), i["rule"].as_str().unwrap(), i["message"].as_str().unwrap())).collect();
    let has = |file: &str, rule: &str, text: &str| assert!(msgs.iter().any(|m| m.starts_with(&format!("{file} {rule} ")) && m.contains(text)), "缺少 {file} {rule} {text}：{msgs:#?}");
    has("04-bad.json", "web-url", "写完整地址");
    has("04-bad.json", "ui-step", "第 1 步：swipe 不能用在 Web 用例里");
    has("04-bad.json", "ui-locator", "第 2 步：定位只写一种（role、text）");
    has("04-bad.json", "ui-step", "第 3 步：fill 要写 value");
    has("04-bad.json", "ui-step", "第 4 步写了多个动作（click、hover）");
    has("04-bad.json", "ui-locator", "第 5 步：Web 用例不能写 id");
    has("04-bad.json", "capture-source", "第 6 步 capture.a");
    has("05-bad.json", "android-app", "写应用包名");
    has("05-bad.json", "ui-step", "第 1 步：select 不能用在 Android 用例里");
    has("05-bad.json", "ui-locator", "第 2 步：Android 用例不能写 css");
    has("05-bad.json", "ui-step", "第 3 步：不认识的按键 Hyper");
    has("05-bad.json", "ui-locator", "第 4 步：state / count / value / hasText 是对元素的要求");

    // Mac 桌面：新建、预览步骤、写错的地方（不运行：不需要 agent-desktop 和被测应用）
    std::fs::remove_file(d.join("collections/ui/04-bad.json")).unwrap();
    std::fs::remove_file(d.join("collections/ui/05-bad.json")).unwrap();
    assert!(run_cli(&["new", "case", "collections/ui", "文本编辑", "--desktop", "TextEdit"], d).status.success());
    let mac: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("collections/ui/04-文本编辑.json")).unwrap()).unwrap();
    assert_eq!((mac["desktop"]["app"].as_str(), mac.get("expect")), (Some("TextEdit"), None));
    std::fs::write(
        d.join("collections/ui/05-写入.json"),
        r#"{"name":"写入","desktop":{"app":"TextEdit","window":"Untitled","steps":[{"click":{"role":"button","name":"New Document"},"optional":true},{"fill":{"role":"textfield"},"value":"{{greeting}}"},{"press":"Meta+s"},{"expect":{"role":"textfield","value":"你好"}},{"wait":1,"capture":{"w":"window"}}]}}"#,
    )
    .unwrap();
    std::fs::write(d.join("envs/dev.json"), r#"{"vars":{"webBase":"http://127.0.0.1:9","greeting":"你好"}}"#).unwrap();
    let out = run_cli(&["validate", "--report", "json"], d);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["issues"], serde_json::json!([]), "{v:#}");
    let out = run_cli(&["run", "collections/ui/05-写入.json", "--dry-run", "-v"], d);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("#2 填写 输入框 = 「你好」") && text.contains("#3 按键 Meta+s") && text.contains("MAC"), "{text}");
    write("06-bad.json", r#"{"desktop":{"app":"","steps":[{"hover":{"text":"x"}},{"click":{"css":".x"}},{"press":"Hyper+"},{"swipe":"up"},{"wait":1,"capture":{"a":"activity"}},{"tap":{"text":"a"}}]}}"#);
    let out = run_cli(&["validate", "--report", "json"], d);
    assert_eq!(out.status.code(), Some(2));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let msgs: Vec<String> = v["issues"].as_array().unwrap().iter().map(|i| format!("{} {} {}", i["file"].as_str().unwrap().rsplit('/').next().unwrap(), i["rule"].as_str().unwrap(), i["message"].as_str().unwrap())).collect();
    let has = |file: &str, rule: &str, text: &str| assert!(msgs.iter().any(|m| m.starts_with(&format!("{file} {rule} ")) && m.contains(text)), "缺少 {file} {rule} {text}：{msgs:#?}");
    has("06-bad.json", "desktop-app", "desktop.app 不能为空");
    has("06-bad.json", "ui-step", "第 1 步：hover 不能用在 Mac 桌面 用例里");
    has("06-bad.json", "ui-locator", "第 2 步：Mac 桌面 用例不能写 css");
    has("06-bad.json", "ui-step", "第 3 步：不认识的按键 Hyper+");
    has("06-bad.json", "capture-source", "第 5 步 capture.a：写 \"window\" / \"app\"");
}

/// 真实 Mac 应用跑桌面用例：TextEdit 里输入、读回、追加、断言，每一步截图。
/// 会启动并操作 TextEdit（需要 macOS、agent-desktop 与辅助功能 / 屏幕录制权限），所以默认不跑：
/// `cargo test -p requestbaby --test integration -- --ignored desktop`
#[test]
#[ignore]
fn desktop_case_drives_real_textedit_with_step_screenshots() {
    if requestbaby_core::engine::desktop::find_agent_desktop(None).is_err() {
        eprintln!("跳过：没有安装 agent-desktop");
        return;
    }
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    std::fs::write(d.join("requestbaby.json"), r#"{"version":1,"defaultEnv":"dev","timeoutMs":15000}"#).unwrap();
    std::fs::create_dir_all(d.join("envs")).unwrap();
    std::fs::write(d.join("envs/dev.json"), r#"{"vars":{"greeting":"你好 requestbaby"}}"#).unwrap();
    std::fs::create_dir_all(d.join("collections/mac")).unwrap();
    std::fs::write(
        d.join("collections/mac/01-输入.json"),
        r#"{"name":"TextEdit 输入并读回","desktop":{"app":"TextEdit","steps":[
{"click":{"role":"button","name":"New Document"},"optional":true,"timeoutMs":1500},
{"waitFor":{"role":"textfield"}},
{"fill":{"role":"textfield"},"value":"{{greeting}}"},
{"expect":{"role":"textfield","value":"{{greeting}}"}},
{"type":"，追加"},
{"expect":{"role":"textfield","value":"，追加"}},
{"wait":1,"capture":{"body":{"role":"textfield"}}},
{"press":"Meta+w"},
{"click":{"role":"button","name":"Delete"},"optional":true,"timeoutMs":3000},
{"stop":true}
]}}"#,
    )
    .unwrap();
    let out = run_cli(&["run", "--report", "json"], d);
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.status.success(), "{text}\n{}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    let c = &v["results"][0];
    assert_eq!((c["passed"].as_bool(), c["protocol"].as_str()), (Some(true), Some("desktop")), "{c:#}");
    let ev = &c["response"]["evidence"];
    assert!(ev["steps"].as_array().unwrap().iter().all(|s| s["status"] != "failed"), "{ev:#}");
    assert!(ev["environment"].as_array().unwrap().iter().any(|e| e[0] == "系统"), "{ev:#}");
    // 截图在证据目录里，并且是裁掉阴影后的窗口图
    let first = ev["steps"].as_array().unwrap().iter().find_map(|s| s["screenshot"].as_str()).expect("至少一张截图");
    let shot = d.join(ev["dir"].as_str().unwrap()).join(first);
    assert!(shot.is_file(), "{}", shot.display());
}

/// 真实浏览器跑 Web 用例（本机没有 Chrome / Edge / Chromium 时跳过）：页面是临时目录里的 file://，不起服务。
/// 每一步截图、失败现场、证据进运行记录与 HTML 报告
#[test]
fn web_case_runs_in_real_browser_with_step_screenshots() {
    if requestbaby_core::engine::cdp::find_browser(None).is_err() {
        eprintln!("跳过：本机没有 Chrome / Edge / Chromium");
        return;
    }
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    std::fs::write(d.join("requestbaby.json"), r#"{"version":1,"defaultEnv":"dev"}"#).unwrap();
    std::fs::create_dir_all(d.join("site")).unwrap();
    std::fs::create_dir_all(d.join("collections/web")).unwrap();
    std::fs::write(
        d.join("site/login.html"),
        r#"<!doctype html><meta charset="utf-8"><title>登录</title>
<label>手机号 <input id="p"></label><label><input type="checkbox" id="a"> 同意</label>
<button id="b" disabled>登录</button><button>取消</button><button>取消</button>
<script>const c=()=>{b.disabled=!(p.value.length===11&&a.checked)};p.oninput=c;a.onchange=c;b.onclick=()=>{location.href='home.html#'+p.value}</script>"#,
    )
    .unwrap();
    std::fs::write(d.join("site/home.html"), r#"<!doctype html><meta charset="utf-8"><title>首页</title><h1>欢迎</h1><p id="who" data-testid="who"></p><script>who.textContent='用户 '+location.hash.slice(1)</script>"#).unwrap();
    let base = format!("file://{}", d.join("site").display());
    std::fs::create_dir_all(d.join("envs")).unwrap();
    std::fs::write(d.join("envs/dev.json"), serde_json::json!({ "vars": { "site": base } }).to_string()).unwrap();
    std::fs::write(
        d.join("collections/web/01-登录.json"),
        r#"{"name":"登录","web":{"url":"{{site}}/login.html","steps":[
            {"expect":{"role":"button","name":"登录","state":"disabled"}},
            {"fill":{"label":"手机号"},"value":"13800000000"},
            {"click":{"role":"checkbox","name":"同意"}},
            {"click":{"role":"button","name":"登录"}},
            {"waitFor":{"title":"首页"}},
            {"expect":{"testId":"who","hasText":"13800000000"},"capture":{"who":{"testId":"who","regex":"\\d+"}}}
        ]},"expect":{"body":{"title":"首页"}}}"#,
    )
    .unwrap();
    std::fs::write(
        d.join("collections/web/02-歧义.json"),
        r#"{"name":"歧义","web":{"url":"{{site}}/login.html","timeoutMs":800,"steps":[{"click":{"role":"button","name":"取消"}}]}}"#,
    )
    .unwrap();
    let out = run_cli(&["run", "--report", "json"], d);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|_| panic!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)));
    let ok = &v["results"][0];
    assert_eq!(ok["passed"], true, "{ok:#}");
    let ev = &ok["response"]["evidence"];
    let steps = ev["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 8, "打开 + 6 步 + 结束画面：{ev:#}");
    assert!(steps.iter().all(|s| s["screenshot"].is_string()), "{ev:#}");
    assert!(steps[6]["detail"].as_str().unwrap().contains("who = 13800000000"), "{ev:#}");
    let evdir = d.join(ev["dir"].as_str().unwrap());
    assert!(ev["dir"].as_str().unwrap().starts_with(".requestbaby/runs/"), "{ev:#}");
    for s in steps {
        let f = evdir.join(s["screenshot"].as_str().unwrap());
        assert!(std::fs::read(&f).unwrap().starts_with(&[0xFF, 0xD8]), "截图应为 JPEG：{}", f.display());
    }
    assert!(ev["environment"].as_array().unwrap().iter().any(|e| e[0] == "浏览器"), "{ev:#}");

    let bad = &v["results"][1];
    assert_eq!(bad["passed"], false);
    assert!(bad["tests"][0]["detail"].as_str().unwrap().contains("找到 2 个按钮「取消」"), "{bad:#}");
    let ev = &bad["response"]["evidence"];
    let failed = ev["steps"].as_array().unwrap().iter().find(|s| s["status"] == "failed").unwrap();
    assert!(failed["screenshot"].as_str().unwrap().ends_with("-failed.jpg"));
    let files: Vec<&str> = ev["artifacts"].as_array().unwrap().iter().map(|a| a["file"].as_str().unwrap()).collect();
    assert!(files.contains(&"page.html") && files.contains(&"accessibility.txt"), "{files:?}");

    // HTML 报告内嵌截图
    let out = run_cli(&["run", "collections/web/01-登录.json", "--report", "html"], d);
    assert!(out.status.success());
    let html = std::fs::read_to_string(d.join("report.html")).unwrap();
    assert!(html.contains("data:image/jpeg;base64,") && html.contains("步骤明细") && html.contains("openLB"));

    // 实时进度：每做完一步就有一条 uiStep 事件，且都在该用例的 caseFinished 之前（不必等用例结束）
    let out = run_cli(&["run", "collections/web/01-登录.json", "--json-stream"], d);
    let events: Vec<serde_json::Value> = String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let kinds: Vec<&str> = events.iter().map(|e| e["event"].as_str().unwrap_or_default()).collect();
    let finished = kinds.iter().position(|k| *k == "caseFinished").expect("有 caseFinished");
    let steps: Vec<&serde_json::Value> = events.iter().take(finished).filter(|e| e["event"] == "uiStep").collect();
    assert!(steps.len() >= 3, "{kinds:?}");
    assert_eq!(steps[0]["step"]["index"], 0);
    assert!(steps.iter().all(|e| e["file"] == "collections/web/01-登录.json" && e["dir"].as_str().is_some_and(|d| !d.is_empty())));
    // 实时事件与最终结果里的步骤一致
    let final_steps = events[finished]["result"]["response"]["evidence"]["steps"].as_array().unwrap().len();
    assert_eq!(steps.len(), final_steps);
}

/// 循环 / 条件 / 随机等待 / 本机命令回调：真实浏览器里点计数按钮，repeat 点到 until 满足就停，if 走 then 分支用 exec 写文件；
/// 循环里只保留最后一轮的步骤记录，exec 命令失败算失败（optional 则跳过）
#[test]
fn web_repeat_if_wait_range_and_exec_callbacks() {
    if requestbaby_core::engine::cdp::find_browser(None).is_err() {
        eprintln!("跳过：本机没有 Chrome / Edge / Chromium");
        return;
    }
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    std::fs::write(d.join("requestbaby.json"), r#"{"version":1,"defaultEnv":"dev"}"#).unwrap();
    std::fs::create_dir_all(d.join("site")).unwrap();
    std::fs::create_dir_all(d.join("collections/web")).unwrap();
    std::fs::create_dir_all(d.join("envs")).unwrap();
    std::fs::write(
        d.join("site/count.html"),
        r#"<!doctype html><meta charset="utf-8"><title>计数</title><button id="b">加一</button><p id="n">0</p><p id="flag"></p>
<script>b.onclick=()=>{n.textContent=String(+n.textContent+1);if(+n.textContent>=3)flag.textContent='到'+'了'}</script>"#,
    )
    .unwrap();
    let base = format!("file://{}", d.join("site").display());
    std::fs::write(d.join("envs/dev.json"), serde_json::json!({ "vars": { "site": base } }).to_string()).unwrap();
    std::fs::write(
        d.join("collections/web/01-循环.json"),
        r#"{"name":"循环","web":{"url":"{{site}}/count.html","steps":[
            {"repeat":{"times":10,"var":"k","until":{"text":"到了"},"steps":[
                {"click":{"role":"button","name":"加一"}},
                {"wait":{"min":10,"max":30}}
            ]}},
            {"expect":{"text":"到了"}},
            {"if":{"when":{"text":"到了"},
                "then":[{"exec":{"command":"echo \"$MSG\" > \"$REQUESTBABY_ROOT/hit.txt\"; echo \"$REQUESTBABY_EVIDENCE\" > \"$REQUESTBABY_ROOT/evidence.txt\"","env":{"MSG":"第{{k}}轮命中"}}}],
                "else":[{"exec":"exit 1"}]}},
            {"if":{"when":{"text":"不存在的文字"},"then":[{"exec":"exit 1"}],"else":[{"wait":1}]}}
        ]}}"#,
    )
    .unwrap();
    std::fs::write(
        d.join("collections/web/02-次数.json"),
        r#"{"name":"次数","web":{"url":"{{site}}/count.html","steps":[
            {"repeat":{"times":2,"steps":[{"click":{"role":"button","name":"加一"}}]}},
            {"expect":{"text":"2"}}
        ]}}"#,
    )
    .unwrap();
    std::fs::write(
        d.join("collections/web/03-命令失败.json"),
        r#"{"name":"命令失败","web":{"url":"{{site}}/count.html","steps":[{"exec":"echo boom; exit 3"}]}}"#,
    )
    .unwrap();
    std::fs::write(
        d.join("collections/web/04-命令失败可跳过.json"),
        r#"{"name":"命令失败可跳过","web":{"url":"{{site}}/count.html","steps":[{"exec":"exit 3","optional":true},{"expect":{"text":"0"}}]}}"#,
    )
    .unwrap();
    std::fs::write(
        d.join("collections/web/05-跳出.json"),
        r#"{"name":"跳出","web":{"url":"{{site}}/count.html","steps":[
            {"repeat":{"times":10,"steps":[
                {"click":{"role":"button","name":"加一"}},
                {"if":{"when":{"text":"到了"},"then":[{"break":true}]}}
            ]}},
            {"expect":{"text":"到了"}}
        ]}}"#,
    )
    .unwrap();
    let out = run_cli(&["run", "--report", "json"], d);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|_| panic!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)));
    let by = |name: &str| v["results"].as_array().unwrap().iter().find(|r| r["name"] == name).unwrap_or_else(|| panic!("没有 {name}")).clone();

    // 循环：until 在第 3 轮满足；then 分支的命令拿到了轮次变量
    let c = by("循环");
    assert_eq!(c["passed"], true, "{c:#}");
    assert_eq!(std::fs::read_to_string(d.join("hit.txt")).unwrap().trim(), "第3轮命中");
    // 证据目录是绝对路径，且就是本用例的证据目录（通知里要附截图时用）
    let evidence = std::fs::read_to_string(d.join("evidence.txt")).unwrap();
    let evidence = Path::new(evidence.trim());
    assert!(evidence.is_absolute() && evidence.is_dir() && evidence.join("00-open.jpg").is_file(), "{}", evidence.display());
    let steps = c["response"]["evidence"]["steps"].as_array().unwrap().clone();
    let summary = steps.iter().find(|s| s["action"] == "repeat").expect("循环汇总");
    assert!(summary["title"].as_str().unwrap().contains("共 3 次") && summary["title"].as_str().unwrap().contains("until"), "{summary:#}");
    // 前两轮的步骤记录被丢掉，只剩最后一轮：1 次点击 + 1 次随机等待
    assert_eq!(steps.iter().filter(|s| s["action"] == "click").count(), 1, "{steps:#?}");
    let wait = steps.iter().find(|s| s["action"] == "wait").unwrap();
    assert!(wait["detail"].as_str().unwrap().contains("本次等了"), "{wait:#}");
    assert!(steps.iter().any(|s| s["action"] == "exec" && s["detail"].as_str().is_some_and(|d| d.contains("命令退出码 0"))), "{steps:#?}");
    assert!(steps.iter().any(|s| s["action"] == "if" && s["detail"].as_str().is_some_and(|d| d.contains("条件不满足"))), "{steps:#?}");
    // 步骤序号不重复（截图文件名由它生成）
    let idx: Vec<u64> = steps.iter().map(|s| s["index"].as_u64().unwrap()).collect();
    let mut uniq = idx.clone();
    uniq.sort();
    uniq.dedup();
    assert_eq!(idx.len(), uniq.len(), "{idx:?}");

    // 只写 times：点满次数就结束
    let c = by("次数");
    assert_eq!(c["passed"], true, "{c:#}");
    let steps = c["response"]["evidence"]["steps"].as_array().unwrap();
    assert!(steps.iter().any(|s| s["title"].as_str().is_some_and(|t| t.contains("共 2 次") && t.contains("times"))), "{steps:#?}");

    // break：if 里发现目标就结束循环（第 3 轮），不会点满 10 次
    let c = by("跳出");
    assert_eq!(c["passed"], true, "{c:#}");
    let steps = c["response"]["evidence"]["steps"].as_array().unwrap();
    assert!(steps.iter().any(|s| s["title"].as_str().is_some_and(|t| t.contains("共 3 次") && t.contains("break"))), "{steps:#?}");

    // 命令失败：用例失败，原因里有退出码和输出；optional 则跳过
    let c = by("命令失败");
    assert_eq!(c["passed"], false);
    let why = c["tests"][0]["detail"].as_str().unwrap();
    assert!(why.contains("命令退出码 3") && why.contains("boom"), "{c:#}");
    let c = by("命令失败可跳过");
    assert_eq!(c["passed"], true, "{c:#}");
    assert!(c["response"]["evidence"]["steps"].as_array().unwrap().iter().any(|s| s["status"] == "skipped"), "{c:#}");

    // 实时进度：循环里的步骤不逐条推送，改为最外层循环每开始一轮发一条 loopProgress（界面 / 终端靠它看出还在跑）
    let out = run_cli(&["run", "collections/web/01-循环.json", "--json-stream"], d);
    let events: Vec<serde_json::Value> = String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let progress: Vec<&serde_json::Value> = events.iter().filter(|e| e["event"] == "loopProgress").collect();
    assert_eq!(progress.iter().map(|e| e["round"].as_u64().unwrap()).collect::<Vec<_>>(), vec![1, 2, 3], "{events:#?}");
    assert!(progress[2]["text"].as_str().unwrap().contains("第 3/10 轮") && progress[2]["file"] == "collections/web/01-循环.json", "{:#}", progress[2]);
    // 循环里的点击没有逐条推送（只有 exec / 失败 / 循环外的步骤）
    let clicks = events.iter().filter(|e| e["event"] == "uiStep" && e["step"]["action"] == "click").count();
    assert_eq!(clicks, 0, "{events:#?}");
}

/// 写法有误的循环 / 命令：validate 报错，不会跑起来无限循环
#[test]
fn repeat_if_exec_authoring_mistakes_are_caught_by_validate() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    std::fs::write(d.join("requestbaby.json"), r#"{"version":1,"defaultEnv":"dev"}"#).unwrap();
    std::fs::create_dir_all(d.join("collections/web")).unwrap();
    std::fs::create_dir_all(d.join("envs")).unwrap();
    std::fs::write(d.join("envs/dev.json"), r#"{"vars":{}}"#).unwrap();
    std::fs::write(
        d.join("collections/web/01-错.json"),
        r#"{"name":"错","web":{"url":"https://example.com","steps":[
            {"repeat":{"steps":[{"click":{"text":"a"}}]}},
            {"wait":{"min":9,"max":1}},
            {"exec":"echo {{x}}"},
            {"if":{"when":{"text":"a"}}},
            {"repeat":{"times":2,"steps":[{"exec":""}]}},
            {"break":true}
        ]}}"#,
    )
    .unwrap();
    let out = run_cli(&["validate", "--report", "json"], d);
    assert_eq!(out.status.code(), Some(2), "{}", String::from_utf8_lossy(&out.stdout));
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    for want in ["之一", "min（9）不能大于 max（1）", "不做变量替换", "if 没有要执行的步骤", "exec 要写命令", "repeat 里的第 1 步", "break 只能写在 repeat 里面"] {
        assert!(text.contains(want), "缺少「{want}」：{text}");
    }
}

/// 简化写法：`cycle` 轮换取值、`"3s-8s"` / `"2s"` 这样的人话时长、项目里命名的命令、用例失败时的 `onFail` 通知
#[test]
fn web_cycle_human_durations_named_commands_and_on_fail() {
    if requestbaby_core::engine::cdp::find_browser(None).is_err() {
        eprintln!("跳过：本机没有 Chrome / Edge / Chromium");
        return;
    }
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    // 命名命令只在项目里写一次，用例里 {"exec": "note"} 引用，值走 env
    std::fs::write(
        d.join("requestbaby.json"),
        serde_json::json!({ "version": 1, "defaultEnv": "dev", "commands": { "note": "echo \"$MSG\" >> \"$REQUESTBABY_ROOT/notes.txt\"" } }).to_string(),
    )
    .unwrap();
    std::fs::create_dir_all(d.join("site")).unwrap();
    std::fs::create_dir_all(d.join("collections/web")).unwrap();
    std::fs::create_dir_all(d.join("envs")).unwrap();
    std::fs::write(
        d.join("site/count.html"),
        r#"<!doctype html><meta charset="utf-8"><title>计数</title><button id="b">加一</button><p id="n">0</p><p id="flag"></p>
<script>b.onclick=()=>{n.textContent=String(+n.textContent+1);if(+n.textContent>=3)flag.textContent='到'+'了'}</script>"#,
    )
    .unwrap();
    let base = format!("file://{}", d.join("site").display());
    std::fs::write(d.join("envs/dev.json"), serde_json::json!({ "vars": { "site": base } }).to_string()).unwrap();
    std::fs::write(
        d.join("collections/web/01-轮换.json"),
        r#"{"name":"轮换","web":{"url":"{{site}}/count.html","steps":[
            {"repeat":{"cycle":{"who":["甲","乙","丙"]},"steps":[
                {"click":{"role":"button","name":"加一"}},
                {"exec":{"command":"note","env":{"MSG":"轮到{{who}}"}}},
                {"wait":"10ms-30ms"}
            ]}},
            {"expect":{"text":"到了"}}
        ]}}"#,
    )
    .unwrap();
    std::fs::write(
        d.join("collections/web/02-人话时长.json"),
        r#"{"name":"人话时长","web":{"url":"{{site}}/count.html","timeout":"5s","steps":[
            {"repeat":{"for":"3s","until":{"text":"到了"},"steps":[
                {"click":{"role":"button","name":"加一"}},
                {"wait":"20ms"}
            ]}},
            {"expect":{"text":"到了"},"timeout":"2s"}
        ]}}"#,
    )
    .unwrap();
    std::fs::write(
        d.join("collections/web/03-失败通知.json"),
        r#"{"name":"失败通知","web":{"url":"{{site}}/count.html",
            "onFail":[{"exec":{"command":"note","env":{"MSG":"失败：{{error}}"}}}],
            "steps":[{"expect":{"text":"这段文字不存在"},"timeout":"300ms"}]}}"#,
    )
    .unwrap();
    std::fs::write(
        d.join("collections/web/04-成功不通知.json"),
        r#"{"name":"成功不通知","web":{"url":"{{site}}/count.html",
            "onFail":[{"exec":{"command":"note","env":{"MSG":"不该出现"}}}],
            "steps":[{"expect":{"text":"加一"}}]}}"#,
    )
    .unwrap();
    let out = run_cli(&["run", "--report", "json"], d);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|_| panic!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)));
    let by = |name: &str| v["results"].as_array().unwrap().iter().find(|r| r["name"] == name).unwrap_or_else(|| panic!("没有 {name}")).clone();

    // cycle：每个值各一轮（共 3 轮），命名命令拿到了 {{who}}
    let c = by("轮换");
    assert_eq!(c["passed"], true, "{c:#}");
    let steps = c["response"]["evidence"]["steps"].as_array().unwrap();
    assert!(steps.iter().any(|s| s["title"].as_str().is_some_and(|t| t.contains("共 3 次") && t.contains("轮换的每个值都轮过了"))), "{steps:#?}");
    // 人话时长：for / timeout / wait 都能写文字
    let c = by("人话时长");
    assert_eq!(c["passed"], true, "{c:#}");
    // onFail：失败了才通知，带失败原因；通知本身不改变用例结论
    let c = by("失败通知");
    assert_eq!(c["passed"], false, "{c:#}");
    let steps = c["response"]["evidence"]["steps"].as_array().unwrap();
    assert!(steps.iter().any(|s| s["title"].as_str().is_some_and(|t| t.starts_with("失败回调：")) && s["status"] == "passed"), "{steps:#?}");
    assert_eq!(by("成功不通知")["passed"], true);

    let notes = std::fs::read_to_string(d.join("notes.txt")).unwrap();
    let lines: Vec<&str> = notes.lines().collect();
    assert_eq!(&lines[..3], ["轮到甲", "轮到乙", "轮到丙"], "{notes}");
    assert!(lines.iter().any(|l| l.starts_with("失败：") && l.contains("这段文字不存在")), "{notes}");
    assert!(!notes.contains("不该出现"), "{notes}");
}

/// 简化写法写错了：validate 提前说清楚（区域名没定义、onFail 里写了别的动作、cycle 没有值、看不懂的时长）
#[test]
fn new_authoring_shortcuts_report_mistakes_in_validate() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    std::fs::write(d.join("requestbaby.json"), r#"{"version":1,"defaultEnv":"dev","commands":{"note":"echo hi"}}"#).unwrap();
    std::fs::create_dir_all(d.join("collections/a")).unwrap();
    std::fs::create_dir_all(d.join("envs")).unwrap();
    std::fs::write(d.join("envs/dev.json"), r#"{"vars":{}}"#).unwrap();
    std::fs::write(
        d.join("collections/a/01-错.json"),
        r#"{"name":"错","regions":{"条":{"x":0,"y":0.1,"w":1,"h":0.1}},
            "android":{"app":"com.tencent.mm","attach":true,
              "onFail":[{"tap":{"text":"x"}}],
              "steps":[
                {"expect":{"ocr":"已约满","region":"没起过的名字"}},
                {"expect":{"ocr":"已约满","region":"条"}},
                {"repeat":{"cycle":{"d":[]},"for":"8h","steps":[{"wait":"1s"}]}}
              ]}}"#,
    )
    .unwrap();
    let out = run_cli(&["validate", "--report", "json"], d);
    assert_eq!(out.status.code(), Some(2), "{}", String::from_utf8_lossy(&out.stdout));
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    for want in ["没起过的名字", "没有定义", "已定义：条", "失败回调里只能写 exec", "cycle「d」没有值"] {
        assert!(text.contains(want), "缺少「{want}」：{text}");
    }
    assert!(!text.contains("region「条」"), "已定义的区域名不该报错：{text}");

    std::fs::write(
        d.join("collections/a/01-错.json"),
        r#"{"name":"错","android":{"app":"com.tencent.mm","attach":true,"steps":[{"wait":"三秒"}]}}"#,
    )
    .unwrap();
    let out = run_cli(&["validate", "--report", "json"], d);
    let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    assert!(text.contains("看不懂时长") && text.contains("2.5s"), "{text}");
}

/// fmt 把界面用例的别名统一成规范写法：Android 点击写 tap、Web / Mac 写 click、时长写人话；改写后再格式化不变，validate 仍然通过
#[test]
fn fmt_fix_rewrites_ui_aliases_to_one_canonical_form() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    std::fs::write(d.join("requestbaby.json"), r#"{"version":1,"defaultEnv":"dev"}"#).unwrap();
    std::fs::create_dir_all(d.join("collections/ui")).unwrap();
    std::fs::create_dir_all(d.join("envs")).unwrap();
    std::fs::write(d.join("envs/dev.json"), r#"{"vars":{}}"#).unwrap();
    std::fs::write(
        d.join("collections/ui/01-旧写法.json"),
        r#"{"name":"旧写法","android":{"app":"com.tencent.mm","attach":true,"timeoutMs":15000,"steps":[
            {"click":{"text":"登录"},"timeoutMs":2500},
            {"wait":2500},
            {"wait":{"min":3000,"max":8000}},
            {"repeat":{"forMs":28800000,"times":3,"steps":[{"click":{"text":"刷新"}},{"wait":500}]}}
        ]}}"#,
    )
    .unwrap();
    std::fs::write(
        d.join("collections/ui/02-网页.json"),
        r#"{"name":"网页","web":{"url":"https://example.com","timeoutMs":20000,"steps":[{"tap":{"text":"登录"}},{"wait":1500}]}}"#,
    )
    .unwrap();
    let out = run_cli(&["fmt", "--check"], d);
    assert_eq!(out.status.code(), Some(1), "{}", String::from_utf8_lossy(&out.stdout));
    assert!(String::from_utf8_lossy(&out.stdout).matches("可精简").count() >= 2);
    let out = run_cli(&["fmt", "--fix"], d);
    assert!(out.status.success(), "{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let a: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("collections/ui/01-旧写法.json")).unwrap()).unwrap();
    assert_eq!(a["android"]["timeout"], "15s");
    assert_eq!(a["android"]["steps"][0], serde_json::json!({"tap": {"text": "登录"}, "timeout": "2.5s"}));
    assert_eq!((&a["android"]["steps"][1], &a["android"]["steps"][2]), (&serde_json::json!({"wait": "2.5s"}), &serde_json::json!({"wait": "3s-8s"})));
    assert_eq!(a["android"]["steps"][3]["repeat"]["for"], "8h");
    assert_eq!(a["android"]["steps"][3]["repeat"]["steps"][0], serde_json::json!({"tap": {"text": "刷新"}}));
    let w: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("collections/ui/02-网页.json")).unwrap()).unwrap();
    assert_eq!((&w["web"]["timeout"], &w["web"]["steps"][0], &w["web"]["steps"][1]), (&serde_json::json!("20s"), &serde_json::json!({"click": {"text": "登录"}}), &serde_json::json!({"wait": "1.5s"})));
    // 改写后：再检查是干净的，validate 仍然通过
    let out = run_cli(&["fmt", "--check"], d);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
    let out = run_cli(&["validate", "--report", "json"], d);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["errors"], 0, "{v:#}");
}

/// 实时步骤事件与日志同样脱敏：填进页面的 secret 变量值（手机号 / 密码）不能出现在事件流里
#[test]
fn web_live_step_events_redact_secret_values() {
    if requestbaby_core::engine::cdp::find_browser(None).is_err() {
        eprintln!("跳过：本机没有 Chrome / Edge / Chromium");
        return;
    }
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    std::fs::write(d.join("requestbaby.json"), r#"{"version":1,"defaultEnv":"dev","vars":{"phone":{"value":"13800000000","secret":true}}}"#).unwrap();
    std::fs::create_dir_all(d.join("site")).unwrap();
    std::fs::create_dir_all(d.join("collections/web")).unwrap();
    std::fs::create_dir_all(d.join("envs")).unwrap();
    std::fs::write(d.join("site/login.html"), r#"<!doctype html><meta charset="utf-8"><title>登录</title><label>手机号 <input id="p"></label>"#).unwrap();
    let base = format!("file://{}", d.join("site").display());
    std::fs::write(d.join("envs/dev.json"), serde_json::json!({ "vars": { "site": base } }).to_string()).unwrap();
    std::fs::write(
        d.join("collections/web/01-登录.json"),
        r#"{"name":"登录","web":{"url":"{{site}}/login.html","steps":[{"fill":{"label":"手机号"},"value":"{{phone}}"}]}}"#,
    )
    .unwrap();
    let out = run_cli(&["run", "collections/web/01-登录.json", "--json-stream"], d);
    let stream = String::from_utf8_lossy(&out.stdout);
    assert!(stream.contains("uiStep"), "{stream}");
    assert!(!stream.contains("13800000000"), "secret 值泄露到事件流：{stream}");
    assert!(stream.contains("****"), "{stream}");
}

/// 多端混合：同一个流程里先跑 API，再跑 Web；API 捕获的值被 Web 用例填进页面（数据在端与端之间传递），
/// 导出的 HTML 是一个整体：时间线按先后排列两个端、脚本输出随用例带上、失败现场的文件内容直接嵌进报告
#[test]
fn mixed_api_and_web_share_data_and_export_one_report_with_timeline() {
    if requestbaby_core::engine::cdp::find_browser(None).is_err() {
        eprintln!("跳过：本机没有 Chrome / Edge / Chromium");
        return;
    }
    let port = spawn_json_api();
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    std::fs::create_dir_all(d.join("site")).unwrap();
    std::fs::create_dir_all(d.join("envs")).unwrap();
    std::fs::create_dir_all(d.join("collections/10-api")).unwrap();
    std::fs::create_dir_all(d.join("collections/20-web")).unwrap();
    std::fs::write(
        d.join("requestbaby.json"),
        r#"{"version":1,"defaultEnv":"dev","workflows":{"mixed":{"steps":[{"run":"collections/10-api"},{"run":"collections/20-web"}]}}}"#,
    )
    .unwrap();
    std::fs::write(d.join("site/form.html"), r#"<!doctype html><meta charset="utf-8"><title>表单</title><label>用户编号 <input id="u"></label><button>提交</button>"#).unwrap();
    let base = format!("file://{}", d.join("site").display());
    std::fs::write(d.join("envs/dev.json"), serde_json::json!({ "vars": { "apiBase": format!("http://127.0.0.1:{port}"), "site": base } }).to_string()).unwrap();
    std::fs::write(
        d.join("collections/10-api/01-登录.json"),
        r#"{"name":"登录","request":{"method":"GET","url":"{{apiBase}}/login"},"expect":{"status":200},"capture":{"uid":"data.uid"},"post":"log('登录完成')"}"#,
    )
    .unwrap();
    std::fs::write(
        d.join("collections/20-web/01-填写.json"),
        r#"{"name":"网页填写","web":{"url":"{{site}}/form.html","steps":[{"fill":{"label":"用户编号"},"value":"{{uid}}"},{"expect":{"label":"用户编号","value":"42"}}]}}"#,
    )
    .unwrap();
    std::fs::write(
        d.join("collections/20-web/02-找不到.json"),
        r#"{"name":"找不到按钮","web":{"url":"{{site}}/form.html","timeoutMs":600,"steps":[{"click":{"role":"button","name":"不存在"}}]}}"#,
    )
    .unwrap();
    let out = run_cli(&["workflow", "mixed", "--report", "json"], d);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|_| panic!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)));
    let r = v["results"].as_array().unwrap();
    assert_eq!(r[0]["passed"], true, "{}", r[0]);
    assert_eq!(r[1]["passed"], true, "API 捕获的 uid 应传到 Web 用例：{}", r[1]);
    assert_eq!(r[2]["passed"], false);
    assert!(r.iter().all(|c| c["finished_at_ms"].as_u64().is_some_and(|t| t > 0)), "每个用例都记录了结束时刻");

    let out_dir = d.join("out");
    std::fs::create_dir_all(&out_dir).unwrap();
    let html_path = out_dir.join("all.html");
    assert_eq!(run_cli(&["export", "-o", html_path.to_str().unwrap()], d).status.code(), Some(0));
    let html = std::fs::read_to_string(&html_path).unwrap();
    assert!(html.contains("\"timeline\"") && html.contains("\"sides\""), "报告里有时间线与端的列表");
    assert!(html.contains("\"artifactText\"") && html.contains("accessibility.txt"), "失败现场的内容嵌进了报告");
    assert!(html.contains("登录完成"), "脚本输出带进了报告");
    assert!(html.contains("id=\"v-tl\"") || html.contains("v-tl"), "页面带「时间线」切换");
    // 嵌进去的失败现场是真实内容（无障碍树里有页面上的按钮），不只是文件名
    let art = html.split("\"artifactText\"").nth(1).unwrap();
    assert!(art.contains("提交"), "{}", &art[..art.len().min(400)]);

    // 纯文本导出：结论 + 按先后排的多端时间线 + 失败详情，API 在前、Web 在后
    let txt_path = out_dir.join("all.txt");
    assert_eq!(run_cli(&["export", "-f", "text", "-o", txt_path.to_str().unwrap()], d).status.code(), Some(0));
    let txt = std::fs::read_to_string(&txt_path).unwrap();
    let (api, web) = (txt.find("[API]").expect("有 API 行"), txt.find("[Web]").expect("有 Web 行"));
    assert!(api < web && txt.contains("时间线") && txt.contains("失败详情") && txt.contains("找不到按钮"), "{txt}");
}

/// 流程依赖与登录态：登录放在一个流程里，业务流程用 needs 依赖它，登录令牌在 localStorage 里（不是 Cookie），
/// 业务流程不用重复登录；步骤的 `as` 切换用户，每个用户登录态独立；没有依赖的流程是干净的；
/// verify 里依赖相同的流程只登录一次
#[test]
fn workflow_needs_share_login_state_across_flows_and_users() {
    if requestbaby_core::engine::cdp::find_browser(None).is_err() {
        eprintln!("跳过：本机没有 Chrome / Edge / Chromium");
        return;
    }
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    for sub in ["site", "envs", "flows/auth", "flows/biz", "flows/guest"] {
        std::fs::create_dir_all(d.join(sub)).unwrap();
    }
    std::fs::write(
        d.join("site/login.html"),
        r#"<!doctype html><meta charset="utf-8"><title>登录</title><label>用户名 <input id="n"></label><button id="b">登录</button>
<script>b.onclick=()=>{localStorage.setItem('token','tk-'+n.value);location.href='app.html'}</script>"#,
    )
    .unwrap();
    std::fs::write(
        d.join("site/app.html"),
        r#"<!doctype html><meta charset="utf-8"><title>首页</title><p id="who"></p><script>who.textContent=localStorage.getItem('token')?'已登录 '+localStorage.getItem('token'):'未登录'</script>"#,
    )
    .unwrap();
    let base = format!("file://{}", d.join("site").display());
    std::fs::write(d.join("envs/dev.json"), serde_json::json!({ "vars": { "site": base } }).to_string()).unwrap();
    let login = |who: &str| format!(r#"{{"name":"{who}登录","web":{{"url":"{{{{site}}}}/login.html","steps":[{{"fill":{{"label":"用户名"}},"value":"{who}"}},{{"click":{{"role":"button","name":"登录"}}}},{{"waitFor":{{"text":"已登录 tk-{who}"}}}}]}}}}"#);
    let see = |name: &str, text: &str| format!(r#"{{"name":"{name}","web":{{"url":"{{{{site}}}}/app.html","timeoutMs":3000,"steps":[{{"expect":{{"text":"{text}"}}}}]}}}}"#);
    std::fs::write(d.join("flows/auth/01-teacher.json"), login("teacher")).unwrap();
    std::fs::write(d.join("flows/auth/02-student.json"), login("student")).unwrap();
    std::fs::write(d.join("flows/biz/01-老师看首页.json"), see("老师看首页", "已登录 tk-teacher")).unwrap();
    std::fs::write(d.join("flows/biz/02-学生看首页.json"), see("学生看首页", "已登录 tk-student")).unwrap();
    std::fs::write(d.join("flows/guest/01-访客.json"), see("访客看首页", "未登录")).unwrap();
    std::fs::write(
        d.join("requestbaby.json"),
        serde_json::json!({
            "version": 1, "defaultEnv": "dev",
            "workflows": {
                "login-teacher": { "steps": [{ "run": "flows/auth/01-teacher.json" }] },
                "login-student": { "steps": [{ "run": "flows/auth/02-student.json", "as": "student" }] },
                "biz": { "needs": ["login-teacher"], "steps": [{ "run": "flows/biz/01-老师看首页.json" }] },
                "biz3": { "needs": ["login-teacher"], "steps": [{ "run": "flows/biz/01-老师看首页.json" }] },
                "biz2": {
                    "needs": ["login-teacher", "login-student"],
                    "steps": [
                        { "run": "flows/biz/01-老师看首页.json" },
                        { "run": "flows/biz/02-学生看首页.json", "as": "student" },
                        { "run": "flows/biz/01-老师看首页.json" }
                    ]
                },
                "guest": { "steps": [{ "run": "flows/guest/01-访客.json" }] }
            }
        })
        .to_string(),
    )
    .unwrap();
    let names = |args: &[&str]| -> (i32, Vec<String>) {
        let out = run_cli(args, d);
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|_| panic!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)));
        let list = v["results"].as_array().map(|r| r.iter().map(|c| format!("{}{}", if c["passed"] == true { "✔" } else { "✘" }, c["name"].as_str().unwrap_or_default())).collect()).unwrap_or_default();
        (out.status.code().unwrap_or(-1), list)
    };

    assert_eq!(run_cli(&["validate"], d).status.code(), Some(0), "{}", String::from_utf8_lossy(&run_cli(&["validate"], d).stderr));
    // 业务流程不自己写登录：依赖的登录流程先跑，令牌在 localStorage 里也被沿用
    let (code, r) = names(&["workflow", "biz", "--report", "json"]);
    assert_eq!((code, r), (0, vec!["✔teacher登录".into(), "✔老师看首页".into()]));
    // 不跑依赖：没有登录态，业务步骤失败
    let (code, r) = names(&["workflow", "biz", "--no-needs", "--report", "json"]);
    assert_eq!((code, r), (1, vec!["✘老师看首页".into()]));
    // 切用户：老师、学生各有各的登录态，来回切换互不影响
    let (code, r) = names(&["workflow", "biz2", "--report", "json"]);
    assert_eq!(code, 0, "{r:?}");
    assert_eq!(r, ["✔teacher登录", "✔student登录", "✔老师看首页", "✔学生看首页", "✔老师看首页"]);
    // 没有依赖的流程是干净的
    assert_eq!(names(&["workflow", "guest", "--report", "json"]), (0, vec!["✔访客看首页".into()]));

    // 管理命令：依赖的增删、重命名跟随、被依赖时不能删、成环被拒
    assert_eq!(run_cli(&["workflow", "needs", "biz3", "add", "login-student"], d).status.code(), Some(0));
    assert_eq!(run_cli(&["workflow", "needs", "biz3", "rm", "login-student"], d).status.code(), Some(0));
    assert_eq!(run_cli(&["workflow", "rm", "login-teacher"], d).status.code(), Some(2));
    assert_eq!(run_cli(&["workflow", "needs", "login-teacher", "add", "biz"], d).status.code(), Some(2));
    let show = String::from_utf8_lossy(&run_cli(&["workflow", "show", "biz2"], d).stdout).to_string();
    assert!(show.contains("依赖 login-teacher → login-student") && show.contains("用户 student"), "{show}");

    // verify：依赖相同（都只依赖 login-teacher）的流程共用同一次登录；没有依赖的流程仍是干净的
    let out = run_cli(&["verify", "--report", "json"], d);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|_| panic!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)));
    assert_eq!(out.status.code(), Some(0), "{} {}", v["blocked"], v["issues"]);
    let total_of = |label: &str| v["steps"].as_array().unwrap().iter().find(|s| s["label"] == label).map(|s| s["total"].as_u64().unwrap()).unwrap_or_else(|| panic!("没有阶段 {label}：{}", v["steps"]));
    assert_eq!(total_of("流程 biz"), 2, "biz 自己跑登录");
    assert_eq!(total_of("流程 biz3"), 1, "biz3 与 biz 依赖相同，沿用登录态，不再登录");
    assert_eq!(total_of("流程 guest"), 1);
}

/// 远程浏览器：另起一个开着调试端口的 Chrome（相当于 Docker / 另一台机器上的浏览器），用 web.endpoint 连接
#[test]
fn web_case_runs_on_remote_browser_via_endpoint() {
    let Ok(chrome) = requestbaby_core::engine::cdp::find_browser(None) else {
        eprintln!("跳过：本机没有 Chrome / Edge / Chromium");
        return;
    };
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let mut browser = Command::new(&chrome)
        .args([
            &format!("--remote-debugging-port={port}"),
            &format!("--user-data-dir={}", d.join("profile").display()),
            "--headless=new",
            "--no-first-run",
            "--no-default-browser-check",
            "about:blank",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    struct Kill<'a>(&'a mut std::process::Child);
    impl Drop for Kill<'_> {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let browser = Kill(&mut browser);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(std::time::Instant::now() < deadline, "远程浏览器没有就绪");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    std::fs::write(d.join("requestbaby.json"), serde_json::json!({ "version": 1, "defaultEnv": "dev", "web": { "endpoint": format!("http://127.0.0.1:{port}") } }).to_string()).unwrap();
    std::fs::create_dir_all(d.join("collections/web")).unwrap();
    std::fs::write(d.join("page.html"), r#"<!doctype html><meta charset="utf-8"><title>远程</title><button onclick="this.textContent='已点'">点我</button>"#).unwrap();
    std::fs::write(
        d.join("collections/web/远程.json"),
        serde_json::json!({ "name": "远程", "web": { "url": format!("file://{}", d.join("page.html").display()), "steps": [
            { "click": { "role": "button", "name": "点我" } },
            { "expect": { "role": "button", "name": "已点" } }
        ] } })
        .to_string(),
    )
    .unwrap();
    let out = run_cli(&["run", "--report", "json"], d);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|_| panic!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)));
    let r = &v["results"][0];
    assert_eq!(r["passed"], true, "{r:#}");
    let env = r["response"]["evidence"]["environment"].as_array().unwrap();
    assert!(env.iter().any(|e| e[0] == "模式" && e[1].as_str().unwrap().starts_with("远程")), "{env:?}");

    // 运行结束只断开连接，远程浏览器还活着
    assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_ok(), "不应关闭远程浏览器");
    let out = run_cli(&["doctor"], d);
    let text = strip_ansi(&String::from_utf8_lossy(&out.stdout));
    assert!(text.contains(&format!("远程浏览器 http://127.0.0.1:{port}")), "{text}");
    drop(browser);
}

/// 代理设置：CLI 读写 requestbaby.json（与桌面端项目设置同一实现），校验指出错误地址，--proxy / --no-proxy 覆盖本次
#[test]
fn proxy_setting_round_trips_and_is_validated() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    std::fs::write(d.join("requestbaby.json"), r#"{"version":1,"defaultEnv":"dev"}"#).unwrap();
    let read = || serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(d.join("requestbaby.json")).unwrap()).unwrap();

    let out = run_cli(&["proxy", "on"], d);
    assert!(!out.status.success(), "还没有地址时不能启用");

    let out = run_cli(&["proxy", "set", "socks5h://u:p@127.0.0.1:9", "--bypass", "corp.internal, 10.0.0.0/8"], d);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(!String::from_utf8_lossy(&out.stdout).contains("u:p"), "输出不含账号密码");
    assert_eq!(read()["proxy"], serde_json::json!({ "enabled": true, "url": "socks5h://u:p@127.0.0.1:9", "noProxy": ["corp.internal", "10.0.0.0/8"] }));

    // 代理不可用：proxy 状态报错，doctor 指出
    let out = run_cli(&["proxy"], d);
    assert!(!out.status.success());
    assert!(strip_ansi(&String::from_utf8_lossy(&out.stderr)).contains("连不上代理 socks5h://127.0.0.1:9"), "{}", String::from_utf8_lossy(&out.stderr));
    let text = strip_ansi(&String::from_utf8_lossy(&run_cli(&["doctor"], d).stdout));
    assert!(text.contains("代理 socks5h://127.0.0.1:9 不可用"), "{text}");

    // 禁用保留地址；再启用
    assert!(run_cli(&["proxy", "off"], d).status.success());
    assert_eq!(read()["proxy"]["enabled"], false);
    assert_eq!(read()["proxy"]["url"], "socks5h://u:p@127.0.0.1:9");
    let text = strip_ansi(&String::from_utf8_lossy(&run_cli(&["doctor"], d).stdout));
    assert!(text.contains("代理：已禁用（全部直连）"), "{text}");
    assert!(run_cli(&["proxy", "on"], d).status.success());
    assert_eq!(read()["proxy"]["enabled"], true);

    // 本次覆盖
    let text = strip_ansi(&String::from_utf8_lossy(&run_cli(&["doctor", "--no-proxy"], d).stdout));
    assert!(text.contains("代理：已禁用（全部直连）（REQUESTBABY_PROXY）"), "{text}");
    let out = run_cli(&["proxy", "--proxy", "ftp://x:1"], d);
    assert!(strip_ansi(&String::from_utf8_lossy(&out.stderr)).contains("代理协议 ftp 不支持"), "{}", String::from_utf8_lossy(&out.stderr));

    // 删除 = 跟随系统
    assert!(run_cli(&["proxy", "unset"], d).status.success());
    assert!(read().get("proxy").is_none());

    // 校验：地址不合法 / 启用了却没写地址
    std::fs::write(d.join("requestbaby.json"), r#"{"version":1,"proxy":{"url":"127.0.0.1:1080"}}"#).unwrap();
    let out = run_cli(&["validate"], d);
    assert!(String::from_utf8_lossy(&out.stdout).contains("proxy-url"), "{}", String::from_utf8_lossy(&out.stdout));
    std::fs::write(d.join("requestbaby.json"), r#"{"version":1,"proxy":{"enabled":true}}"#).unwrap();
    let out = run_cli(&["validate"], d);
    assert!(String::from_utf8_lossy(&out.stdout).contains("proxy 已启用但没写 url"), "{}", String::from_utf8_lossy(&out.stdout));
}
