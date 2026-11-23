mod term;

use clap::{Args, Parser, Subcommand};
use requestbaby_core::varstore::{self, VarDef, VarScope};
use requestbaby_core::{config, doctor, envs, fmt, guide, history, importer, inspect, protocols, report, runner, scaffold, session, validate, verify, workflow};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

#[derive(Parser)]
#[command(
    name = "requestbaby",
    version,
    about = "Independent acceptance layer for AI-delivered work: JSON test cases for HTTP / gRPC / WebSocket APIs and Web / Android UIs, with step-by-step screenshot evidence",
    after_help = "官网与下载：https://apps.zhizuzhefu.com/requestbaby/　反馈：https://github.com/zhizuzhefu/requestbaby/issues",
    long_about = "requestbaby — AI 交付的独立验收层，辅助人对 AI 交付的成果验收：在接口边界核对行为，一个 JSON 文件一个用例。\nAI / LLM 对接：requestbaby guide（用例格式 + 调度协议）；配置契约：requestbaby schema --all。"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
    /// 本次经这个代理访问（socks5:// / socks5h:// / http://，覆盖 requestbaby.json 的 proxy）
    #[arg(long, global = true, value_name = "URL", conflicts_with = "no_proxy")]
    proxy: Option<String>,
    /// 本次不使用代理：全部直连（也不读 http_proxy 等环境变量）
    #[arg(long, global = true)]
    no_proxy: bool,
}

#[derive(Subcommand)]
enum Cmd {
    /// 初始化项目脚手架
    Init {
        /// 目标目录（缺省当前目录）
        path: Option<String>,
    },
    /// 新建用例 / 集合 / 环境（与 Desktop 新建同一模板）
    New {
        #[command(subcommand)]
        what: NewCmd,
    },
    /// 格式化并精简用例：重复 headers 上提集合级、字段规范排序、清理空字段；--fix 按 validate 的写法建议自动改写
    Fmt {
        /// 目标目录（缺省 collections）
        paths: Vec<String>,
        /// 只检查不修改（CI 用：有可精简项则 exit 1 并列出）
        #[arg(long)]
        check: bool,
        /// 同时按写法建议改写：JS 断言 → expect、post 取值 → capture、expectStatus → expect.status、写死的域名 → {{baseUrl}} 等
        #[arg(long)]
        fix: bool,
    },
    /// 环境自检（AI 调度前置）：引擎 / 配置完整性 / 环境 / 用例 / 历史
    Doctor {
        /// 检查 baseUrl 可达性所用的环境（缺省 defaultEnv）
        #[arg(short, long)]
        env: Option<String>,
        /// 输出 JSON
        #[arg(long)]
        json: bool,
    },
    /// 使用文档（与本版本一致，人和 AI 都读这一份）：缺省输出速查卡；guide <主题> 按场景深入
    Guide {
        /// 主题：expect capture vars flow auth e2e data setup env grpc ws web android desktop scripts cli ref all（也接受中文：断言 / 捕获 / 流程 …）
        topic: Option<String>,
        /// 在项目根写入 AGENTS.md / CLAUDE.md 入口（已存在的跳过）
        #[arg(long)]
        init: bool,
        /// 与 --init 一起用：旧版 requestbaby 生成的 AGENTS.md 更新为当前版本（你自己写的不覆盖）
        #[arg(long, requires = "init")]
        update: bool,
    },
    /// gRPC：列出服务的方法、查看请求消息骨架（来源：requestbaby.json 的 grpc.protos，未配置则服务端反射）
    Grpc {
        #[command(subcommand)]
        action: GrpcCmd,
    },
    /// Web 界面：snapshot 打开页面，列出可操作的元素与可直接粘贴的定位（写步骤前先看一眼）
    Web {
        #[command(subcommand)]
        action: WebCmd,
    },
    /// Android 界面：devices 列出设备；snapshot 列出当前界面的元素与可直接粘贴的定位
    Android {
        #[command(subcommand)]
        action: AndroidCmd,
    },
    /// Mac 桌面应用：apps 列出正在运行的应用；snapshot 列出应用窗口里的元素与可直接粘贴的定位（需要 agent-desktop）
    Desktop {
        #[command(subcommand)]
        action: DesktopCmd,
    },
    /// 关于：版本、官网、源码、文档与反馈地址
    About,
    /// 检查并升级到最新版本（下载 GitHub Release 的安装包，校验 SHA256 后替换自身）
    Upgrade {
        /// 只检查有没有新版本，不安装（有新版本时 exit 1，便于脚本判断）
        #[arg(long)]
        check: bool,
    },
    /// （内部）桌面端自动更新的替换程序：等 App 退出后原子替换并复检，由 App / `upgrade` 自动调用，不需要手动运行
    #[command(name = "__apply-update", hide = true)]
    ApplyUpdate {
        #[arg(long)]
        pid: u32,
        #[arg(long)]
        app: PathBuf,
        #[arg(long)]
        staged: PathBuf,
        #[arg(long)]
        version: String,
        #[arg(long)]
        relaunch: bool,
        #[arg(long, default_value_t = 180)]
        wait_secs: u64,
    },
    /// 代理：所有对外连接（HTTP / gRPC / WebSocket / 浏览器 / adb）经同一个代理。不带参数显示当前设置并检查代理
    Proxy {
        #[command(subcommand)]
        action: Option<ProxyCmd>,
    },
    /// 导出 JSON Schema（--all 输出 case/collection/env/project bundle）
    Schema {
        /// 全部四类
        #[arg(long)]
        all: bool,
    },
    /// 校验配置：Schema 结构 + 变量引用 + 脚本语法 + 引用存在性
    Validate {
        /// 要校验的文件/目录（缺省 collections）
        paths: Vec<String>,
        /// 环境名（决定参与变量检查的 env 文件）
        #[arg(short, long)]
        env: Option<String>,
        /// 报告格式：term（默认）/ json（file/level/rule/message 四元组）
        #[arg(long)]
        report: Option<String>,
    },
    /// 列出可执行的用例
    List {
        paths: Vec<String>,
    },
    /// 变量：全项目总览（缺省）/ 用例解析预览 / 设置 / 删除（与 Desktop 变量面板同一实现）
    #[command(args_conflicts_with_subcommands = true)]
    Vars {
        #[command(subcommand)]
        action: Option<VarsCmd>,
        /// 用例文件：预览该用例最终使用的变量值、来源层级、运行时捕获与未定义引用
        case: Option<String>,
        #[arg(short, long)]
        env: Option<String>,
        /// 输出 JSON
        #[arg(long)]
        json: bool,
    },
    /// 导入外部格式：OpenAPI 3 / Swagger 2 / Postman Collection v2.1（JSON 或 YAML）
    Import {
        /// spec 文件路径（OpenAPI/Swagger/Postman JSON 或 YAML）
        spec: Option<String>,
        /// 直接导入 curl 命令（与 spec 二选一）
        #[arg(long)]
        curl: Option<String>,
        /// 输出目录（生成完整项目脚手架，缺省 ./imported）
        #[arg(short, long)]
        out: Option<String>,
        /// 环境名（缺省 dev）
        #[arg(short, long)]
        env: Option<String>,
    },
    /// 交互式发送单个用例：展示状态/响应头/美化响应体/断言（调试用）
    Send {
        /// 用例文件
        case: String,
        /// 环境名
        #[arg(short, long)]
        env: Option<String>,
        /// 按真实响应列出可直接粘贴进 tests 的断言（字段 = 当前值 / 类型），以及 post 捕获语句
        #[arg(long)]
        suggest: bool,
        /// 把响应体原样保存到文件（图片 / 文件下载等二进制响应用）
        #[arg(long, value_name = "FILE")]
        save_body: Option<String>,
        /// Web 用例显示浏览器窗口（缺省无界面运行）
        #[arg(long)]
        headed: bool,
    },
    /// 流程：`workflow <key>` 运行；list / show / new / rename / cp / rm / step 管理（与 Desktop 流程编辑器同一实现）
    /// 定义在 requestbaby.json 的 workflows 字段；多阶段编排，变量跨步骤自动传递
    #[command(args_conflicts_with_subcommands = true)]
    Workflow {
        #[command(subcommand)]
        action: Option<WorkflowCmd>,
        /// 要运行的流程 key
        name: Option<String>,
        #[arg(short, long)]
        env: Option<String>,
        /// 编译 curl 不执行
        #[arg(long)]
        dry_run: bool,
        #[arg(short, long)]
        verbose: bool,
        /// 报告格式：term / json / html
        #[arg(long)]
        report: Option<String>,
        /// 逐行输出运行事件（NDJSON，含 stepStarted / stepFinished）
        #[arg(long)]
        json_stream: bool,
        /// 不写入运行历史
        #[arg(long)]
        no_history: bool,
        /// 不运行依赖的流程（needs）：只跑本流程自己的步骤
        #[arg(long)]
        no_needs: bool,
    },
    /// 环境：list / show / new / rename / cp / rm / default（与 Desktop 环境管理页同一实现）
    Env {
        #[command(subcommand)]
        action: Option<EnvCmd>,
    },
    /// 用例文件：new / rename / cp / rm（与 Desktop 集合树右键同一实现；删除移到废纸篓）
    Case {
        #[command(subcommand)]
        action: CaseCmd,
    },
    /// 集合目录：new / rename / cp / rm（与 Desktop 集合树右键同一实现；删除移到废纸篓）
    Collection {
        #[command(subcommand)]
        action: CollectionCmd,
    },
    /// 完整验证：校验 → 全部用例 → 全部流程。一个结论、一个退出码（0 通过 / 1 用例失败 / 2 无法完成验证）
    Verify {
        #[arg(short, long)]
        env: Option<String>,
        /// 报告格式：term（默认）/ json（合并报告：steps = 阶段，issues = 校验问题，blocked = 无法完成的原因）
        #[arg(long)]
        report: Option<String>,
        /// 响应落盘目录（每用例一个 JSON：meta + body）
        #[arg(short, long)]
        output: Option<String>,
        /// 逐行输出运行事件（NDJSON；阶段即 stepStarted / stepFinished）
        #[arg(long)]
        json_stream: bool,
        /// 不写入运行历史
        #[arg(long)]
        no_history: bool,
    },
    /// 监听配置变更自动重跑（开发时挂终端）
    Watch {
        paths: Vec<String>,
        #[arg(short, long)]
        env: Option<String>,
        #[arg(long)]
        tag: Vec<String>,
        #[arg(long)]
        name: Option<String>,
    },
    /// 运行用例 / 集合
    Run {
        /// 文件 / 目录
        paths: Vec<String>,
        /// 环境名（缺省用 requestbaby.json 的 defaultEnv，再缺省 dev）
        #[arg(short, long)]
        env: Option<String>,
        /// 标签过滤（可多次，任一命中即选）
        #[arg(long)]
        tag: Vec<String>,
        /// 用例名子串过滤
        #[arg(long)]
        name: Option<String>,
        /// 只重跑最近失败的用例（按运行历史；自动带上它依赖的前置用例，如捕获 token 的登录）
        #[arg(long)]
        failed: bool,
        /// 首个失败立即中止
        #[arg(long)]
        fail_fast: bool,
        /// 编译出 curl 命令打印，不执行
        #[arg(long)]
        dry_run: bool,
        /// 详细输出：curl 命令 / 全部断言 / 脚本日志
        #[arg(short, long)]
        verbose: bool,
        /// 响应落盘目录（每用例一个 JSON：meta + body）
        #[arg(short, long)]
        output: Option<String>,
        /// 独立用例并发档位（依赖捕获链/带脚本的仍串行在前；不支持 setup/teardown）
        #[arg(long)]
        parallel: Option<usize>,
        /// 报告格式：term（默认）/ json / html（写 report.html）
        #[arg(long)]
        report: Option<String>,
        /// 逐行输出运行事件（NDJSON，与 Desktop 同一事件协议），替代常规报告
        #[arg(long)]
        json_stream: bool,
        /// 不写入运行历史（.requestbaby/runs/）
        #[arg(long)]
        no_history: bool,
        /// Web 用例显示浏览器窗口（缺省无界面运行；也可设 REQUESTBABY_HEADED=1）
        #[arg(long)]
        headed: bool,
    },
    /// 导出验收结果交给别人：缺省 = 全部用例的最新结果，一份 HTML（截图已嵌入，直接用浏览器打开）；也可导出某一次运行（与桌面端「导出结果」同一实现）
    Export {
        /// 运行 ID：只导出这一次运行（ID 见 requestbaby history）；缺省导出环境下每个用例的最新结果
        id: Option<String>,
        /// 环境名（缺省用 requestbaby.json 的 defaultEnv，再缺省 dev）；指定运行 ID 时忽略
        #[arg(short, long)]
        env: Option<String>,
        /// 只导出这些集合（如 -c 03-布置作业 -c 04-布置测验，可省略 collections/，可多次）：每个用例的全部步骤与截图在同一份 HTML 里
        #[arg(short, long)]
        collection: Vec<String>,
        /// 格式：html（缺省）/ json / text（纯文本：结论 + 多端时间线 + 失败详情，可直接贴进聊天或给 AI）
        #[arg(short, long, default_value = "html")]
        format: String,
        /// 输出文件或目录（缺省：当前目录下 验收报告-<项目>-<环境>.html）
        #[arg(short, long)]
        output: Option<String>,
    },
    /// 在桌面端打开项目（需安装 requestbaby-desktop）
    Open {
        /// 项目目录（缺省当前目录）
        path: Option<String>,
    },
    /// 运行历史（CLI / Desktop 共享 .requestbaby/runs/）：查看；rm / clear 清理（与 Desktop 历史面板同一实现）
    #[command(args_conflicts_with_subcommands = true)]
    History {
        #[command(subcommand)]
        action: Option<HistoryCmd>,
        /// 显示条数
        #[arg(short = 'n', long, default_value_t = 10)]
        limit: usize,
        /// 输出 JSON（RunReport 数组）
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum HistoryCmd {
    /// 删除指定的运行记录（ID 见 requestbaby history）
    Rm {
        #[arg(required = true)]
        ids: Vec<String>,
    },
    /// 批量清理：--all 全部；--keep N 只保留最近 N 条；--older-than 7d 删除 7 天前的（可与 --keep 组合）
    Clear {
        #[arg(long, conflicts_with_all = ["keep", "older_than"])]
        all: bool,
        #[arg(long)]
        keep: Option<usize>,
        /// 时长：30m / 12h / 7d
        #[arg(long, value_name = "DURATION")]
        older_than: Option<String>,
    },
}

#[derive(Subcommand)]
enum WebCmd {
    /// 打开页面，列出按钮 / 链接 / 输入框… 与推荐定位，并截一张图
    Snapshot {
        /// 页面地址（支持 {{变量}}，按环境替换）
        url: String,
        #[arg(short, long)]
        env: Option<String>,
        /// 显示浏览器窗口
        #[arg(long)]
        headed: bool,
        /// 同时识别屏幕上的文字（OCR），列出可直接粘贴的 ocr 定位；界面结构里没有文字的页面（微信小程序、自绘控件）用它
        #[arg(long)]
        ocr: bool,
        /// 输出 JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum ProxyCmd {
    /// 设置代理地址并启用
    Set {
        /// socks5://主机:端口 / socks5h://（域名由代理解析）/ http://；可带 user:pass@，可写 {{变量}}
        url: String,
        /// 不走代理的主机，逗号分隔：example.com（含子域）/ *.corp / 10.0.0.0/8
        #[arg(long, value_name = "HOSTS")]
        bypass: Option<String>,
    },
    /// 启用已设置的代理
    On,
    /// 禁用代理：全部直连（保留地址，proxy on 再打开）
    Off,
    /// 删除代理设置，跟随系统（环境变量 HTTPS_PROXY / ALL_PROXY … → 系统代理设置）
    Unset,
}

#[derive(Subcommand)]
enum DesktopCmd {
    /// 列出正在运行的 Mac 应用（名称与 bundle ID，用例的 app 写其中之一）
    Apps,
    /// 读取应用窗口当前的界面，列出元素与推荐定位，并截一张图
    Snapshot {
        /// 应用名称（如 TextEdit）或 bundle ID
        app: String,
        /// 应用有多个窗口时：窗口标题里的一段文字
        #[arg(long)]
        window: Option<String>,
        /// 同时识别屏幕上的文字（OCR），列出可直接粘贴的 ocr 定位；界面结构里没有文字的页面（微信小程序、自绘控件）用它
        #[arg(long)]
        ocr: bool,
        /// 输出 JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum AndroidCmd {
    /// 列出已连接的设备（adb devices）
    Devices,
    /// 读取设备当前界面，列出元素与推荐定位，并截一张图
    Snapshot {
        /// 同时识别屏幕上的文字（OCR），列出可直接粘贴的 ocr 定位；界面结构里没有文字的页面（微信小程序、自绘控件）用它
        #[arg(long)]
        ocr: bool,
        /// 输出 JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)]
enum NewCmd {
    /// 在集合目录下新建用例（自动编号 NN-<名称>.json；同 `case new`）
    Case(NewCaseArgs),
    /// 新建集合目录（含 collection.json；同 `collection new`）
    Collection(NewCollectionArgs),
    /// 新建环境 envs/<name>.json（同 `env new`）
    Env(NewEnvArgs),
}

#[derive(Args)]
struct NewCaseArgs {
    /// 集合目录
    dir: String,
    /// 用例名（--curl 时可省略：取「方法 + 路径末段」）
    name: Option<String>,
    #[arg(short, long, default_value = "GET")]
    method: String,
    #[arg(short, long, default_value = "")]
    url: String,
    /// 从 curl 命令生成（浏览器「Copy as cURL」原样粘贴；`-` 从标准输入读取）。
    /// URL 以当前环境 baseUrl 开头时自动改写为 {{baseUrl}}
    #[arg(long, conflicts_with_all = ["method", "url"])]
    curl: Option<String>,
    /// 新建 gRPC 用例：方法名 包.服务/方法（能连上服务或配置了 .proto 时自动填好请求消息骨架）
    #[arg(long, value_name = "METHOD", conflicts_with_all = ["method", "url", "curl", "ws"])]
    grpc: Option<String>,
    /// 与 --grpc 一起：服务地址（缺省 {{grpcTarget}}）
    #[arg(long, requires = "grpc")]
    target: Option<String>,
    /// 新建 WebSocket 用例：连接地址（缺省 {{wsBase}}/）
    #[arg(long, value_name = "URL", num_args = 0..=1, default_missing_value = "", conflicts_with_all = ["method", "url", "curl"])]
    ws: Option<String>,
    /// 新建 Web 界面用例：要打开的页面（缺省 {{webBase}}/）
    #[arg(long, value_name = "URL", num_args = 0..=1, default_missing_value = "", conflicts_with_all = ["method", "url", "curl", "grpc", "ws"])]
    web: Option<String>,
    /// 新建 Android 界面用例：应用包名
    #[arg(long, value_name = "PACKAGE", conflicts_with_all = ["method", "url", "curl", "grpc", "ws", "web"])]
    android: Option<String>,
    /// 新建 Mac 桌面应用用例：应用名称（如 TextEdit）或 bundle ID
    #[arg(long, value_name = "APP", conflicts_with_all = ["method", "url", "curl", "grpc", "ws", "web", "android"])]
    desktop: Option<String>,
    /// 改写 baseUrl 时参考的环境（缺省 defaultEnv）
    #[arg(short, long)]
    env: Option<String>,
}

#[derive(Args)]
struct NewCollectionArgs {
    /// 父目录
    parent: String,
    name: String,
}

#[derive(Args)]
struct NewEnvArgs {
    name: String,
    /// 复制已有环境的变量（不含本机覆盖）
    #[arg(long)]
    from: Option<String>,
}

#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)]
enum CaseCmd {
    /// 新建用例（同 `new case`）
    New(NewCaseArgs),
    /// 重命名用例文件（同级，自动保留 .json）
    Rename { path: String, name: String },
    /// 复制为同级新用例（name 字段同步改为新名称）
    Cp { path: String, name: String },
    /// 删除用例（移到废纸篓）
    Rm { path: String },
}

#[derive(Subcommand)]
enum CollectionCmd {
    /// 新建集合（同 `new collection`）
    New(NewCollectionArgs),
    /// 重命名集合目录（同级）
    Rename { dir: String, name: String },
    /// 复制整个集合目录（含全部用例）
    Cp { dir: String, name: String },
    /// 删除集合目录及其中全部用例（移到废纸篓）
    Rm { dir: String },
}

#[derive(Subcommand)]
enum EnvCmd {
    /// 列出全部环境（默认 / 变量数 / 本机覆盖 / 固定在它上面的流程）
    List {
        #[arg(long)]
        json: bool,
    },
    /// 查看环境的变量（Secret 脱敏）
    Show {
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// 新建环境（同 `new env`）
    New(NewEnvArgs),
    /// 重命名环境：同步本机覆盖、defaultEnv、固定在它上面的流程
    Rename { name: String, new_name: String },
    /// 复制为新环境（不含本机覆盖）
    Cp { name: String, new_name: String },
    /// 删除环境（连同本机覆盖移到废纸篓）；默认环境 / 被流程固定的环境不能删
    Rm { name: String },
    /// 设为默认环境（requestbaby.json 的 defaultEnv）
    Default { name: String },
}

#[derive(Subcommand)]
enum WorkflowCmd {
    /// 列出全部流程
    List {
        #[arg(long)]
        json: bool,
    },
    /// 查看流程的步骤
    Show {
        key: String,
        #[arg(long)]
        json: bool,
    },
    /// 新建流程（--step 可多次，按顺序成为步骤；之后可用 step add 继续编排）
    New {
        key: String,
        /// 显示名
        #[arg(long)]
        name: Option<String>,
        /// 步骤运行目标（目录或用例，相对项目根）
        #[arg(long = "step")]
        steps: Vec<String>,
        /// 固定运行环境（如本地 fixture 只能跑 local）：verify 会在该环境运行它
        #[arg(short, long)]
        env: Option<String>,
        /// 依赖的流程（可多次，如 --needs login）：先运行它们，登录态沿用到本流程所有步骤
        #[arg(long = "needs")]
        needs: Vec<String>,
    },
    /// 流程依赖：`workflow needs <流程>` 查看；`needs <流程> add <依赖>` / `needs <流程> rm <依赖>`
    Needs {
        key: String,
        #[command(subcommand)]
        action: Option<NeedsCmd>,
    },
    /// 固定 / 取消固定流程的运行环境：`workflow env <key> local`、`workflow env <key> --unset`
    Env {
        key: String,
        /// 环境名
        #[arg(required_unless_present = "unset")]
        env: Option<String>,
        /// 取消固定：跟随 -e / defaultEnv
        #[arg(long, conflicts_with = "env")]
        unset: bool,
    },
    /// 重命名 key 和 / 或修改显示名
    Rename {
        key: String,
        /// 新 key（缺省不变）
        new_key: Option<String>,
        /// 新显示名（传空字符串清除）
        #[arg(long)]
        name: Option<String>,
    },
    /// 复制为新流程
    Cp { key: String, new_key: String },
    /// 删除流程
    Rm { key: String },
    /// 步骤编排：add / rm / mv（位置从 1 开始）
    Step {
        #[command(subcommand)]
        action: StepCmd,
    },
}

#[derive(Subcommand)]
enum GrpcCmd {
    /// 列出全部方法（包.服务/方法 · 调用方式 · 请求 → 响应类型）
    #[command(alias = "ls")]
    List {
        /// 服务地址（缺省环境变量 {{grpcTarget}}；配置了 .proto 时不连服务）
        #[arg(default_value = "{{grpcTarget}}")]
        target: String,
        #[arg(short, long)]
        env: Option<String>,
        /// TLS 时跳过证书校验
        #[arg(long)]
        insecure: bool,
        #[arg(long)]
        json: bool,
    },
    /// 方法详情与请求消息骨架（可直接粘贴进用例）
    Describe {
        /// 包.服务/方法
        method: String,
        /// 服务地址（缺省 {{grpcTarget}}）
        #[arg(default_value = "{{grpcTarget}}")]
        target: String,
        #[arg(short, long)]
        env: Option<String>,
        #[arg(long)]
        insecure: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum NeedsCmd {
    /// 增加依赖的流程
    Add { need: String },
    /// 去掉依赖的流程
    Rm { need: String },
}

#[derive(Subcommand)]
enum StepCmd {
    /// 添加步骤（缺省追加到末尾）
    Add {
        key: String,
        /// 运行目标：目录或用例（相对项目根）
        run: String,
        /// 步骤名
        #[arg(long)]
        name: Option<String>,
        /// 标签过滤（可多次）
        #[arg(long)]
        tag: Vec<String>,
        /// 用例名过滤
        #[arg(long)]
        name_filter: Option<String>,
        /// 插入位置（从 1 开始）
        #[arg(long)]
        at: Option<usize>,
        /// 以哪个用户运行（如 teacher / student）：每个用户有独立的登录态，换名字 = 切换用户
        #[arg(long = "as")]
        as_user: Option<String>,
    },
    /// 删除第 N 个步骤
    Rm { key: String, index: usize },
    /// 把第 FROM 个步骤移到第 TO 位
    Mv { key: String, from: usize, to: usize },
}

#[derive(Subcommand)]
enum VarsCmd {
    /// 设置变量：写入项目 / 环境 / 集合文件（已存在则覆盖）
    Set {
        name: String,
        /// 值（--from-env 时省略）
        value: Option<String>,
        #[command(flatten)]
        scope: ScopeArgs,
        /// 敏感值：报告 / 日志 / curl 命令中一律脱敏
        #[arg(long, conflicts_with = "from_env")]
        secret: bool,
        /// 运行时读取进程环境变量（缺省与变量同名）
        #[arg(long, value_name = "ENV_NAME", num_args = 0..=1, default_missing_value = "")]
        from_env: Option<String>,
    },
    /// 删除变量
    Unset {
        name: String,
        #[command(flatten)]
        scope: ScopeArgs,
    },
}

/// 变量所在层：三选一
#[derive(Args)]
#[group(required = true, multiple = false)]
struct ScopeArgs {
    /// 环境 envs/<ENV>.json
    #[arg(short, long)]
    env: Option<String>,
    /// 本机覆盖 envs/<ENV>.local.json：密钥 / 本机端口，自动加入 .gitignore，不入库
    #[arg(long, value_name = "ENV")]
    local: Option<String>,
    /// 项目 requestbaby.json（所有环境共享）
    #[arg(long)]
    project: bool,
    /// 集合目录（写入 <DIR>/collection.json）
    #[arg(long, value_name = "DIR")]
    collection: Option<String>,
}

impl ScopeArgs {
    fn scope(&self, root: &std::path::Path) -> VarScope {
        match (&self.env, &self.collection) {
            (Some(e), _) => VarScope::Env(e.clone()),
            _ if self.local.is_some() => VarScope::Local(self.local.clone().unwrap_or_default()),
            (_, Some(dir)) => {
                let abs = std::fs::canonicalize(dir).unwrap_or_else(|_| PathBuf::from(dir));
                let rel = abs.strip_prefix(root).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or_else(|_| dir.clone());
                VarScope::Collection(rel)
            }
            _ => VarScope::Project,
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    // 本次运行的代理覆盖：交给 core 统一解析（与 REQUESTBABY_PROXY 同一入口）
    if cli.no_proxy {
        std::env::set_var(requestbaby_core::engine::proxy::ENV_OVERRIDE, "off");
    } else if let Some(p) = &cli.proxy {
        std::env::set_var(requestbaby_core::engine::proxy::ENV_OVERRIDE, p);
    }
    let code = match cli.cmd {
        Cmd::Init { path } => cmd_init(path.as_deref()),
        Cmd::New { what } => cmd_new(what),
        Cmd::Grpc { action } => cmd_grpc(action),
        Cmd::Web { action } => cmd_web(action),
        Cmd::Android { action } => cmd_android(action),
        Cmd::Desktop { action } => cmd_desktop(action),
        Cmd::Proxy { action } => cmd_proxy(action),
        Cmd::About => {
            println!("{}", requestbaby_core::about::text());
            0
        }
        Cmd::Upgrade { check } => cmd_upgrade(check),
        Cmd::ApplyUpdate { pid, app, staged, version, relaunch, wait_secs } => {
            match requestbaby_core::update_app::apply(&requestbaby_core::update_app::ApplyArgs { pid, app, staged, version, relaunch, wait_secs }) {
                Ok(()) => 0,
                Err(_) => 1,
            }
        }
        Cmd::Env { action } => cmd_env(action.unwrap_or(EnvCmd::List { json: false })),
        Cmd::Case { action: CaseCmd::New(a) } => cmd_new(NewCmd::Case(a)),
        Cmd::Case { action } => cmd_item(action.into()),
        Cmd::Collection { action: CollectionCmd::New(a) } => cmd_new(NewCmd::Collection(a)),
        Cmd::Collection { action } => cmd_item(action.into()),
        Cmd::Fmt { paths, check, fix } => cmd_fmt(&paths, check, fix),
        Cmd::Doctor { env, json } => cmd_doctor(env.as_deref(), json),
        Cmd::Guide { init: true, update, .. } => cmd_guide_init(update),
        Cmd::Guide { topic, init: false, .. } => match guide::topic(topic.as_deref().unwrap_or_default()) {
            Some(text) => {
                println!("{text}");
                0
            }
            None => {
                eprintln!("没有主题 {}，可选：\n{}", topic.unwrap_or_default(), guide::topic_list());
                2
            }
        },
        Cmd::Schema { all } => {
            if all {
                println!(
                    "{}",
                    serde_json::json!({
                        "case": serde_json::from_str::<serde_json::Value>(validate::CASE_SCHEMA).unwrap(),
                        "collection": serde_json::from_str::<serde_json::Value>(validate::COLLECTION_SCHEMA).unwrap(),
                        "env": serde_json::from_str::<serde_json::Value>(validate::ENV_SCHEMA).unwrap(),
                        "project": serde_json::from_str::<serde_json::Value>(validate::PROJECT_SCHEMA).unwrap(),
                    })
                );
            } else {
                println!("{}", validate::CASE_SCHEMA);
            }
            0
        }
        Cmd::Validate { paths, env, report } => cmd_validate(&paths, env.as_deref(), report.as_deref()),
        Cmd::List { paths } => cmd_list(&paths),
        Cmd::Vars { action: Some(a), .. } => cmd_vars_edit(a),
        Cmd::Vars { action: None, case: Some(case), env, json } => cmd_vars(&case, env.as_deref(), json),
        Cmd::Vars { action: None, case: None, json, .. } => cmd_vars_matrix(json),
        Cmd::Import { spec, curl, out, env } => cmd_import(spec.as_deref(), curl.as_deref(), out.as_deref(), env.as_deref()),
        Cmd::Send { case, env, suggest, save_body, headed } => cmd_send(&case, env.as_deref(), suggest, save_body.as_deref(), headed),
        Cmd::Workflow { action: Some(a), .. } => cmd_workflow_manage(a),
        Cmd::Workflow { action: None, name: None, .. } => cmd_workflow_manage(WorkflowCmd::List { json: false }),
        Cmd::Workflow { action: None, name: Some(name), env, dry_run, verbose, report, json_stream, no_history, no_needs } => cmd_workflow(WorkflowArgs {
            no_needs,
            name,
            env,
            dry_run,
            verbose,
            report,
            json_stream,
            no_history,
        }),
        Cmd::Verify { env, report, output, json_stream, no_history } => {
            cmd_verify(env, report.as_deref(), output, json_stream, no_history)
        }
        Cmd::Watch { paths, env, tag, name } => cmd_watch(&paths, env.as_deref(), tag, name.as_deref()),
        Cmd::Run { paths, env, tag, name, failed, fail_fast, dry_run, verbose, output, parallel, report, json_stream, no_history, headed } => {
            let req = session::RunRequest { paths, env, tags: tag, name, failed, fail_fast, dry_run, verbose, parallel, output, headed };
            cmd_run(req, report.as_deref(), json_stream, no_history)
        }
        Cmd::Export { id, env, collection, format, output } => cmd_export(id, env, collection, &format, output),
        Cmd::Open { path } => cmd_open(path.as_deref()),
        Cmd::History { action: Some(a), .. } => cmd_history_clean(a),
        Cmd::History { action: None, limit, json } => cmd_history(limit, json),
    };
    ExitCode::from(code)
}

fn err(e: impl std::fmt::Display) -> u8 {
    eprintln!("\x1b[31m错误: {e}\x1b[0m");
    1
}

/// 无法执行（配置 / 路径 / 环境）：退出码 2，与「用例失败」(1) 区分
fn fatal(e: impl std::fmt::Display) -> u8 {
    err(e);
    2
}

fn cmd_init(path: Option<&str>) -> u8 {
    let dir = PathBuf::from(path.unwrap_or("."));
    match scaffold::init_project(&dir) {
        Ok(entries) => {
            for e in entries {
                if e.created {
                    println!("创建: {}", e.path.display());
                } else {
                    eprintln!("跳过已存在: {}", e.path.display());
                }
            }
            println!("\n完成。试试: requestbaby run {}", dir.join("collections").display());
            0
        }
        Err(e) => err(format!("{e:#}")),
    }
}

fn cmd_guide_init(update: bool) -> u8 {
    let root = match project_root_or_err() {
        Ok(r) => r,
        Err(code) => return code,
    };
    let status = guide::agent_entry_status(&root);
    match guide::write_agent_entry_with(&root, update) {
        Ok(entries) => {
            for e in entries {
                let agents = e.path.file_name().is_some_and(|n| n == "AGENTS.md");
                if e.created && agents && status == guide::AgentEntryStatus::Outdated {
                    println!("已更新: {}", e.path.display());
                } else if e.created {
                    println!("创建: {}", e.path.display());
                } else if agents && status == guide::AgentEntryStatus::Outdated {
                    eprintln!("跳过已存在: {}（是旧版入口：加 --update 更新）", e.path.display());
                } else {
                    eprintln!("跳过已存在: {}", e.path.display());
                }
            }
            0
        }
        Err(e) => err(format!("{e:#}")),
    }
}

/// gRPC 用例：尽量取到请求骨架（反射 / .proto），取不到就写空消息并说明原因
fn new_grpc_case(dir: &str, name: &str, method: &str, target: Option<&str>, env: Option<&str>) -> anyhow::Result<PathBuf> {
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
fn cmd_grpc(action: GrpcCmd) -> u8 {
    let Some(root) = session::project_root(std::path::Path::new(".")) else {
        return err("未找到项目根（requestbaby.json）");
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

fn print_snapshot(snap: &protocols::UiSnapshot, json: bool, next: &str) -> u8 {
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

fn cmd_web(action: WebCmd) -> u8 {
    match action {
        WebCmd::Snapshot { url, env, headed, ocr, json } => {
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let root = session::project_root(&cwd);
            let env = session::resolve_env(std::slice::from_ref(&cwd), env.as_deref());
            match protocols::web_snapshot(root.as_deref(), &env, &url, headed, ocr) {
                Ok(s) => print_snapshot(&s, json, "定位粘贴进步骤：{\"click\": <定位>} / {\"fill\": <定位>, \"value\": \"…\"} / {\"expect\": <定位>}（requestbaby guide web）"),
                Err(e) => err(format!("{e:#}")),
            }
        }
    }
}

fn cmd_upgrade(check: bool) -> u8 {
    use requestbaby_core::update;
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
    let dir = std::env::temp_dir().join(format!("requestbaby-download-{}", std::process::id()));
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

/// 命令行在 requestbaby.app 里：与桌面端同一套自动更新（下载 → 校验 → 退出后原子替换）；命令行随 App 一起更新
fn upgrade_app_bundle(rel: &requestbaby_core::update::Release, proxy: &requestbaby_core::engine::proxy::ProxyPolicy) -> u8 {
    use requestbaby_core::update_app as ua;
    if let Some(why) = ua::current_app().and_then(|a| ua::self_update_blocker(&a)) {
        return err(format!("不能自动更新：{why}；或到 {} 手动下载 DMG", rel.page));
    }
    println!("下载并校验 {}（签名 / 团队 / 公证 / 能运行）…", rel.version);
    let pending = match ua::stage(rel, proxy) {
        Ok(p) => p,
        Err(e) => return err(format!("{e:#}（当前版本没有任何改动）")),
    };
    // 本进程一退出（以及所有 requestbaby 窗口都关掉后）更新程序就会替换；等得久一些，给用户时间退出 App
    if let Err(e) = ua::spawn_apply(&pending, false, 900) {
        return err(format!("{e:#}（新版本已暂存，下次打开桌面端退出时会自动安装）"));
    }
    println!("\x1b[32m✓\x1b[0m {} 已下载并校验。退出所有 requestbaby 窗口后会自动替换（15 分钟内），失败会保留当前版本", pending.version);
    0
}

fn cmd_proxy(action: Option<ProxyCmd>) -> u8 {
    use requestbaby_core::proxy_setting::{self as ps, Mode};
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let Some(root) = session::project_root(&cwd) else { return fatal("不在 requestbaby 项目里（没有 requestbaby.json）") };
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
                    println!("  地址    {}", requestbaby_core::engine::proxy::redact(&c.url));
                }
                if !c.no_proxy.is_empty() {
                    println!("  直连    {}", c.no_proxy.join(", "));
                }
            }
            if std::env::var(requestbaby_core::engine::proxy::ENV_OVERRIDE).is_ok_and(|v| !v.trim().is_empty()) {
                println!("  \x1b[33m本次被 --proxy / --no-proxy / REQUESTBABY_PROXY 覆盖\x1b[0m");
            }
            let env = session::resolve_env(std::slice::from_ref(&root), None);
            return match protocols::proxy(&root, &env) {
                Ok(policy) => {
                    println!("  生效    {}（环境 {env}）", policy.describe());
                    if let requestbaby_core::engine::proxy::ProxyPolicy::On(p) = &policy {
                        match requestbaby_core::engine::proxy::probe(p, std::time::Duration::from_secs(5)) {
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
            _ => Err(anyhow::anyhow!("还没有设置代理地址：requestbaby proxy set socks5://主机:端口")),
        },
        Some(ProxyCmd::Off) => {
            let c = current.clone().unwrap_or(requestbaby_core::config::ProxyConfig { enabled: false, url: String::new(), no_proxy: vec![] });
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
        Some(c) if mode == Mode::On => println!("✓ 代理已启用：{}（requestbaby.json）", requestbaby_core::engine::proxy::redact(&c.url)),
        _ => println!("✓ 代理：{}（requestbaby.json）", mode.label()),
    }
    0
}

fn cmd_android(action: AndroidCmd) -> u8 {
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
                    let remote = if requestbaby_core::engine::android::is_network_serial(&id) { "  （网络）" } else { "" };
                    println!("{id}  {note}{remote}");
                }
                u8::from(problem.is_some())
            }
            Err(e) => err(format!("{e:#}")),
        },
        AndroidCmd::Snapshot { ocr, json } => match protocols::android_snapshot(root.as_deref(), ocr) {
            Ok(s) => print_snapshot(&s, json, "定位粘贴进步骤：{\"tap\": <定位>} / {\"fill\": <定位>, \"value\": \"…\"} / {\"expect\": <定位>}（requestbaby guide android）"),
            Err(e) => err(format!("{e:#}")),
        },
    }
}

fn cmd_desktop(action: DesktopCmd) -> u8 {
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
            Ok(s) => print_snapshot(&s, json, "定位粘贴进步骤：{\"click\": <定位>} / {\"fill\": <定位>, \"value\": \"…\"} / {\"expect\": <定位>}（requestbaby guide desktop）"),
            Err(e) => err(format!("{e:#}")),
        },
    }
}

fn cmd_new(what: NewCmd) -> u8 {
    let created = match what {
        NewCmd::Case(NewCaseArgs { dir, name, curl: Some(curl), env, .. }) => new_case_from_curl(&dir, name.as_deref(), &curl, env.as_deref()),
        NewCmd::Case(NewCaseArgs { name: None, .. }) => Err(anyhow::anyhow!("缺少用例名（或用 --curl 从 curl 命令生成）")),
        NewCmd::Case(NewCaseArgs { dir, name: Some(name), grpc: Some(method), target, env, .. }) => new_grpc_case(&dir, &name, &method, target.as_deref(), env.as_deref()),
        NewCmd::Case(NewCaseArgs { dir, name: Some(name), ws: Some(url), .. }) => scaffold::new_ws_case(&PathBuf::from(dir), &name, &url),
        NewCmd::Case(NewCaseArgs { dir, name: Some(name), web: Some(url), .. }) => scaffold::new_web_case(&PathBuf::from(dir), &name, &url),
        NewCmd::Case(NewCaseArgs { dir, name: Some(name), android: Some(app), .. }) => scaffold::new_android_case(&PathBuf::from(dir), &name, &app),
        NewCmd::Case(NewCaseArgs { dir, name: Some(name), desktop: Some(app), .. }) => scaffold::new_desktop_case(&PathBuf::from(dir), &name, &app),
        NewCmd::Case(NewCaseArgs { dir, name: Some(name), method, url, .. }) => scaffold::new_case(&PathBuf::from(dir), &name, &method, &url),
        NewCmd::Collection(NewCollectionArgs { parent, name }) => scaffold::new_collection(&PathBuf::from(parent), &name),
        NewCmd::Env(NewEnvArgs { name, from }) => match session::project_root(std::path::Path::new(".")) {
            Some(root) => envs::create(&root, &name, from.as_deref()),
            None => Err(anyhow::anyhow!("未找到项目根（requestbaby.json）")),
        },
    };
    match created {
        Ok(p) => {
            println!("\x1b[32m✔\x1b[0m 已创建 {}", p.display());
            0
        }
        Err(e) => err(format!("{e:#}")),
    }
}

/// 统一报告输出（run / workflow 共用）
fn emit_report(fmt: &str, results: &[runner::CaseResult], run_report: &report::RunReport, verbose: bool) {
    match fmt {
        "json" => {
            let mut r = report::RunReport::new(results, verbose, run_report.meta.clone());
            r.steps = run_report.steps.clone();
            r.compare = run_report.compare.clone();
            println!("{}", report::render_json(&r));
        }
        "html" => {
            let path = PathBuf::from("report.html");
            let root = std::env::current_dir().ok().and_then(|d| session::project_root(&d));
            requestbaby_core::fsx::write(&path, report::render_html(run_report, root.as_deref())).ok();
            eprintln!("\x1b[2mHTML 报告: {}\x1b[0m", path.display());
            term::print_summary(results, run_report.compare.as_ref());
        }
        _ => {
            term::print_results(results, verbose);
            term::print_summary(results, run_report.compare.as_ref());
        }
    }
}

fn check_report_format(fmt: &str) -> Result<(), String> {
    match fmt {
        "term" | "json" | "html" => Ok(()),
        other => Err(format!("不支持的报告格式 {other}（可选 term / json / html）")),
    }
}

fn json_stream_hook() -> runner::EventHook {
    Arc::new(|ev: &runner::RunEvent| {
        use std::io::Write;
        let line = serde_json::to_string(ev).unwrap_or_default();
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    })
}

/// 运行事件回调：`--json-stream` 输出全部事件到 stdout；终端里的普通运行实时打印界面用例每一步的进度（stderr，不污染 stdout）
fn run_hook(json_stream: bool, term: bool) -> Option<runner::EventHook> {
    use std::io::IsTerminal;
    if json_stream {
        Some(json_stream_hook())
    } else if term && std::io::stderr().is_terminal() {
        Some(live_steps_hook())
    } else {
        None
    }
}

/// 界面用例（Web / Android）跑得慢：每完成一步就打印一行，不必等用例结束才知道做到哪了
fn live_steps_hook() -> runner::EventHook {
    let last = std::sync::Mutex::new(String::new());
    Arc::new(move |ev: &runner::RunEvent| {
        let mut last = last.lock().unwrap_or_else(|e| e.into_inner());
        match ev {
            runner::RunEvent::CaseStarted { .. } => last.clear(),
            runner::RunEvent::UiStep { file, step, .. } => {
                use requestbaby_core::engine::ui::UiStepStatus;
                if *last != *file {
                    eprintln!("\x1b[2m▶ {file}\x1b[0m");
                    *last = file.clone();
                }
                let mark = match step.status {
                    UiStepStatus::Passed => "\x1b[32m✔\x1b[0m",
                    UiStepStatus::Failed => "\x1b[31m✘\x1b[0m",
                    UiStepStatus::Skipped => "\x1b[33m⇣\x1b[0m",
                };
                let detail = match (&step.detail, step.status) {
                    (Some(d), UiStepStatus::Failed) => format!("\n    \x1b[31m{d}\x1b[0m"),
                    _ => String::new(),
                };
                eprintln!("  {mark} \x1b[2m#{}\x1b[0m {}  \x1b[2m{}ms\x1b[0m{detail}", step.index, step.title, step.duration_ms);
            }
            _ => {}
        }
    })
}

fn run_source() -> &'static str {
    if std::env::var_os("CI").is_some() {
        "ci"
    } else {
        "cli"
    }
}

fn cmd_run(req: session::RunRequest, report: Option<&str>, json_stream: bool, no_history: bool) -> u8 {
    let fmt = report.unwrap_or("term");
    if let Err(e) = check_report_format(fmt) {
        return err(e);
    }
    let mut spec = req.to_spec(std::path::Path::new("."), run_source(), false, None);
    spec.record_history = !no_history;
    // --failed 且历史里没有失败：没有要重跑的，属于好消息，不当错误
    if req.failed {
        if let Some(root) = spec.paths.first().and_then(|p| session::project_root(p)) {
            let env = session::resolve_env(&spec.paths, spec.env.as_deref());
            if requestbaby_core::history::failed_files(&root, &env).is_empty() {
                println!("\x1b[32m✔\x1b[0m 环境 {env} 没有最近失败的用例，无需重跑");
                return 0;
            }
        }
    }
    let hook = run_hook(json_stream, fmt == "term");
    let quiet_header = json_stream || fmt == "json";
    let dry_run = req.dry_run;
    let outcome = session::run(&spec, hook, |env, planned, timeout_ms| {
        if !quiet_header {
            println!(
                "\x1b[1mrequestbaby\x1b[0m \x1b[2menv={env} cases={planned} timeout={timeout_ms}ms{}\x1b[0m",
                if dry_run { " [dry-run]" } else { "" }
            );
        }
    });
    let out = match outcome {
        Ok(o) => o,
        Err(e) => return fatal(format!("{e:#}")),
    };
    if !json_stream {
        emit_report(fmt, &out.results, &out.report, req.verbose || req.dry_run);
        term::print_notices(&out.notices);
    }
    if out.report.failed > 0 { 1 } else { 0 }
}

struct WorkflowArgs {
    no_needs: bool,
    name: String,
    env: Option<String>,
    dry_run: bool,
    verbose: bool,
    report: Option<String>,
    json_stream: bool,
    no_history: bool,
}

fn cmd_workflow(a: WorkflowArgs) -> u8 {
    let fmt = a.report.as_deref().unwrap_or("term");
    if let Err(e) = check_report_format(fmt) {
        return err(e);
    }
    let Some(root) = session::project_root(std::path::Path::new(".")) else {
        return err("未找到项目根（requestbaby.json）");
    };
    let spec = workflow::WorkflowSpec {
        root,
        name: a.name.clone(),
        env: a.env.clone(),
        dry_run: a.dry_run,
        verbose: a.verbose,
        send_mode: false,
        source: run_source().into(),
        record_history: !a.no_history,
        cancel: None,
        artifacts: None,
        skip_needs: a.no_needs,
        base: None,
    };
    let is_term = fmt == "term" && !a.json_stream;
    let hook = run_hook(a.json_stream, fmt == "term");
    let verbose = a.verbose || a.dry_run;
    let mut header_done = false;
    let (name, dry_run) = (a.name.clone(), a.dry_run);
    let outcome = workflow::run(&spec, hook, |phase| {
        if !is_term {
            return;
        }
        match phase {
            workflow::StepPhase::Started(info) => {
                if !header_done {
                    header_done = true;
                    println!(
                        "\x1b[1mworkflow\x1b[0m \x1b[1m{name}\x1b[0m \x1b[2msteps={}{}\x1b[0m",
                        info.total,
                        if dry_run { " [dry-run]" } else { "" }
                    );
                }
                let mut extra = String::new();
                if !info.tags.is_empty() {
                    extra.push_str(&format!(" --tag {}", info.tags.join(",")));
                }
                if !info.name_filter.is_empty() {
                    extra.push_str(&format!(" --name {}", info.name_filter));
                }
                println!("\x1b[2m── step {}/{}: {}{} ({})\x1b[0m", info.index + 1, info.total, info.label, extra, info.planned);
            }
            workflow::StepPhase::Finished(_, results) => term::print_results(results, verbose),
        }
    });
    let out = match outcome {
        Ok(o) => o,
        Err(e) => return fatal(format!("{e:#}")),
    };
    if a.json_stream {
        return if out.report.failed > 0 { 1 } else { 0 };
    }
    if let Some(ix) = out.failed_step {
        if is_term {
            term::print_summary(&out.results, out.report.compare.as_ref());
            term::print_notices(&out.notices);
            eprintln!("\x1b[31m流程在步骤 {}/{} 失败中止\x1b[0m", ix + 1, out.steps.len());
        } else {
            emit_report(fmt, &out.results, &out.report, verbose);
        }
        return 1;
    }
    if is_term {
        term::print_summary(&out.results, out.report.compare.as_ref());
    } else {
        emit_report(fmt, &out.results, &out.report, verbose);
    }
    term::print_notices(&out.notices);
    if out.report.failed > 0 { 1 } else { 0 }
}

fn cmd_workflow_manage(action: WorkflowCmd) -> u8 {
    let root = match project_root_or_err() {
        Ok(r) => r,
        Err(_) => return 2,
    };
    let done = |msg: String| {
        println!("\x1b[32m✔\x1b[0m {msg}");
        0
    };
    let pos = |n: usize| n.checked_sub(1).ok_or_else(|| anyhow::anyhow!("位置从 1 开始"));
    let r: anyhow::Result<u8> = (|| match action {
        WorkflowCmd::List { json } => {
            let all = workflow::list(&root)?;
            if json {
                let v: serde_json::Map<String, serde_json::Value> =
                    all.iter().map(|(k, w)| (k.clone(), serde_json::to_value(w).unwrap_or_default())).collect();
                println!("{}", serde_json::to_string_pretty(&v)?);
            } else if all.is_empty() {
                println!("还没有流程：requestbaby workflow new <key> --step <目录或用例>");
            } else {
                for (k, w) in &all {
                    let name = if w.name.is_empty() { String::new() } else { format!(" \x1b[2m{}\x1b[0m", w.name) };
                    let env = w.env.as_ref().map(|e| format!(" · 环境 {e}")).unwrap_or_default();
                    let needs = if w.needs.is_empty() { String::new() } else { format!(" · 依赖 {}", w.needs.join("、")) };
                    println!("\x1b[1m{k}\x1b[0m{name} \x1b[2m· {} 步{env}{needs}\x1b[0m", w.steps.len());
                }
            }
            Ok(0)
        }
        WorkflowCmd::Show { key, json } => {
            let w = workflow::get(&root, &key)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&w)?);
                return Ok(0);
            }
            println!("\x1b[1m{key}\x1b[0m{}", if w.name.is_empty() { String::new() } else { format!(" · {}", w.name) });
            if let Some(e) = &w.env {
                println!("  \x1b[2m固定环境 {e}\x1b[0m");
            }
            if !w.needs.is_empty() {
                println!("  \x1b[2m依赖 {}（先运行，登录态沿用到下面所有步骤）\x1b[0m", w.needs.join(" → "));
            }
            if w.steps.is_empty() && w.needs.is_empty() {
                println!("  （没有步骤：requestbaby workflow step add {key} <目录或用例>）");
            }
            for (i, st) in w.steps.iter().enumerate() {
                let mut extra = String::new();
                if !st.tags.is_empty() {
                    extra.push_str(&format!(" --tag {}", st.tags.join(",")));
                }
                if !st.name_filter.is_empty() {
                    extra.push_str(&format!(" --name {}", st.name_filter));
                }
                if let Some(u) = &st.as_user {
                    extra.push_str(&format!(" · 用户 {u}"));
                }
                let label = if st.name.is_empty() { String::new() } else { format!("{}  ", st.name) };
                println!("  {}. {label}\x1b[2m{}{extra}\x1b[0m", i + 1, st.run);
            }
            Ok(0)
        }
        WorkflowCmd::New { key, name, steps, env, needs } => {
            let wf = config::Workflow {
                name: name.unwrap_or_default(),
                env,
                needs,
                steps: steps.into_iter().map(|run| config::WorkflowStep { run, ..Default::default() }).collect(),
            };
            workflow::save(&root, None, &key, &wf)?;
            let needs = if wf.needs.is_empty() { String::new() } else { format!("，依赖 {}", wf.needs.join("、")) };
            Ok(done(format!("已新建流程 {key}（{} 步{needs}）", wf.steps.len())))
        }
        WorkflowCmd::Needs { key, action: None } => {
            let w = workflow::get(&root, &key)?;
            if w.needs.is_empty() {
                println!("流程 {key} 没有依赖（requestbaby workflow needs {key} add <流程>）");
            } else {
                println!("流程 {key} 依赖（按顺序先运行）：{}", w.needs.join(" → "));
            }
            Ok(0)
        }
        WorkflowCmd::Needs { key, action: Some(NeedsCmd::Add { need }) } => {
            let needs = workflow::add_need(&root, &key, &need)?;
            Ok(done(format!("流程 {key} 依赖：{}", needs.join("、"))))
        }
        WorkflowCmd::Needs { key, action: Some(NeedsCmd::Rm { need }) } => {
            let needs = workflow::remove_need(&root, &key, &need)?;
            Ok(done(if needs.is_empty() { format!("流程 {key} 不再有依赖") } else { format!("流程 {key} 依赖：{}", needs.join("、")) }))
        }
        WorkflowCmd::Rename { key, new_key, name } => {
            let mut wf = workflow::get(&root, &key)?;
            if let Some(n) = name {
                wf.name = n;
            }
            let new_key = new_key.unwrap_or_else(|| key.clone());
            workflow::save(&root, Some(&key), &new_key, &wf)?;
            Ok(done(if new_key == key { format!("已更新流程 {key}") } else { format!("已重命名 {key} → {new_key}") }))
        }
        WorkflowCmd::Env { key, env, unset } => {
            let mut wf = workflow::get(&root, &key)?;
            wf.env = if unset { None } else { env };
            workflow::save(&root, Some(&key), &key, &wf)?;
            Ok(done(match &wf.env {
                Some(e) => format!("流程 {key} 固定在环境 {e}（verify 会在 {e} 运行它）"),
                None => format!("流程 {key} 跟随 -e / defaultEnv 运行"),
            }))
        }
        WorkflowCmd::Cp { key, new_key } => {
            workflow::duplicate(&root, &key, &new_key)?;
            Ok(done(format!("已复制 {key} → {new_key}")))
        }
        WorkflowCmd::Rm { key } => {
            workflow::delete(&root, &key)?;
            Ok(done(format!("已删除流程 {key}")))
        }
        WorkflowCmd::Step { action: StepCmd::Add { key, run, name, tag, name_filter, at, as_user } } => {
            let step = config::WorkflowStep {
                name: name.unwrap_or_default(),
                run: run.clone(),
                tags: tag.iter().flat_map(|t| t.split(',')).map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect(),
                name_filter: name_filter.unwrap_or_default(),
                as_user: as_user.filter(|u| !u.trim().is_empty()),
            };
            let at = at.map(pos).transpose()?;
            let ix = workflow::add_step(&root, &key, step, at)?;
            Ok(done(format!("流程 {key}：第 {} 步 {run}", ix + 1)))
        }
        WorkflowCmd::Step { action: StepCmd::Rm { key, index } } => {
            let st = workflow::remove_step(&root, &key, pos(index)?)?;
            Ok(done(format!("流程 {key}：已删除第 {index} 步 {}", st.run)))
        }
        WorkflowCmd::Step { action: StepCmd::Mv { key, from, to } } => {
            workflow::move_step(&root, &key, pos(from)?, pos(to)?)?;
            Ok(done(format!("流程 {key}：第 {from} 步 → 第 {to} 位")))
        }
    })();
    r.unwrap_or_else(|e| fatal(format!("{e:#}")))
}

fn cmd_env(action: EnvCmd) -> u8 {
    let root = match project_root_or_err() {
        Ok(r) => r,
        Err(_) => return 2,
    };
    let done = |msg: String| {
        println!("\x1b[32m✔\x1b[0m {msg}");
        0
    };
    let r: anyhow::Result<u8> = (|| match action {
        EnvCmd::List { json } => {
            let all = envs::list(&root)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&all)?);
            } else if all.is_empty() {
                println!("还没有环境：requestbaby env new <名称>");
            }
            for e in all.iter().filter(|_| !json) {
                let mut meta = vec![format!("{} 个变量", e.vars)];
                if let Some(n) = e.local_vars {
                    meta.push(format!("本机覆盖 {n}"));
                }
                if !e.pinned_by.is_empty() {
                    meta.push(format!("固定流程 {}", e.pinned_by.join("、")));
                }
                if let Some(err) = &e.error {
                    meta.push(format!("\x1b[31m{err}\x1b[0m\x1b[2m"));
                }
                let default = if e.is_default { " \x1b[32m默认\x1b[0m" } else { "" };
                let base = e.base_url.as_ref().map(|b| format!("  {b}")).unwrap_or_default();
                println!("\x1b[1m{}\x1b[0m{default}{base} \x1b[2m· {}\x1b[0m", e.name, meta.join(" · "));
            }
            Ok(0)
        }
        EnvCmd::Show { name, json } => {
            let vars = varstore::read(&root, &VarScope::Env(name.clone()))?;
            if !config::env_path(&root, &name).exists() {
                anyhow::bail!("环境不存在: {name}");
            }
            if json {
                let m: serde_json::Map<String, serde_json::Value> = vars.iter().map(|(k, d)| (k.clone(), d.encode())).collect();
                println!("{}", serde_json::to_string_pretty(&m)?);
                return Ok(0);
            }
            println!("\x1b[1m{name}\x1b[0m \x1b[2m{}\x1b[0m", config::env_path(&root, &name).strip_prefix(&root).unwrap_or(&root).display());
            if vars.is_empty() {
                println!("  （没有变量：requestbaby vars set <名称> <值> -e {name}）");
            }
            for (k, d) in &vars {
                println!("  {k} = {}", d.display(k));
            }
            Ok(0)
        }
        EnvCmd::New(NewEnvArgs { name, from }) => {
            envs::create(&root, &name, from.as_deref())?;
            Ok(done(format!("已新建环境 {name}{}", from.map(|f| format!("（复制自 {f}）")).unwrap_or_default())))
        }
        EnvCmd::Rename { name, new_name } => {
            envs::rename(&root, &name, &new_name)?;
            Ok(done(format!("已重命名环境 {name} → {new_name}（本机覆盖、defaultEnv、固定的流程已同步）")))
        }
        EnvCmd::Cp { name, new_name } => {
            envs::duplicate(&root, &name, &new_name)?;
            Ok(done(format!("已复制环境 {name} → {new_name}（本机覆盖不复制）")))
        }
        EnvCmd::Rm { name } => {
            let how = envs::delete(&root, &name)?;
            Ok(done(format!("已删除环境 {name}（{}）", deleted_word(how))))
        }
        EnvCmd::Default { name } => {
            envs::set_default(&root, &name)?;
            Ok(done(format!("默认环境：{name}")))
        }
    })();
    r.unwrap_or_else(|e| fatal(format!("{e:#}")))
}

/// 用例 / 集合的重命名、复制、删除（new 走 cmd_new）
enum ItemOp {
    Rename { path: String, name: String, dir: bool },
    Cp { path: String, name: String, dir: bool },
    Rm { path: String, dir: bool },
}

impl From<CaseCmd> for ItemOp {
    fn from(c: CaseCmd) -> Self {
        match c {
            CaseCmd::Rename { path, name } => ItemOp::Rename { path, name, dir: false },
            CaseCmd::Cp { path, name } => ItemOp::Cp { path, name, dir: false },
            CaseCmd::Rm { path } => ItemOp::Rm { path, dir: false },
            CaseCmd::New(_) => unreachable!("case new 由 cmd_new 处理"),
        }
    }
}

impl From<CollectionCmd> for ItemOp {
    fn from(c: CollectionCmd) -> Self {
        match c {
            CollectionCmd::Rename { dir, name } => ItemOp::Rename { path: dir, name, dir: true },
            CollectionCmd::Cp { dir, name } => ItemOp::Cp { path: dir, name, dir: true },
            CollectionCmd::Rm { dir } => ItemOp::Rm { path: dir, dir: true },
            CollectionCmd::New(_) => unreachable!("collection new 由 cmd_new 处理"),
        }
    }
}

fn cmd_item(op: ItemOp) -> u8 {
    let (path, want_dir) = match &op {
        ItemOp::Rename { path, dir, .. } | ItemOp::Cp { path, dir, .. } | ItemOp::Rm { path, dir } => (PathBuf::from(path), *dir),
    };
    let what = if want_dir { "集合" } else { "用例" };
    if want_dir && !path.is_dir() {
        return fatal(format!("集合目录不存在: {}", path.display()));
    }
    if !want_dir && !(path.is_file() && path.extension().is_some_and(|e| e == "json")) {
        return fatal(format!("用例文件不存在: {}", path.display()));
    }
    let r = match op {
        ItemOp::Rename { name, .. } => scaffold::rename(&path, &name).map(|p| format!("已重命名{what} → {}", p.display())),
        ItemOp::Cp { name, .. } => scaffold::duplicate(&path, &name).map(|p| format!("已复制{what} → {}", p.display())),
        ItemOp::Rm { .. } => scaffold::delete(&path).map(|how| format!("已删除{what} {}（{}）", path.display(), deleted_word(how))),
    };
    match r {
        Ok(msg) => {
            println!("\x1b[32m✔\x1b[0m {msg}");
            0
        }
        Err(e) => fatal(format!("{e:#}")),
    }
}

fn deleted_word(how: scaffold::Deleted) -> &'static str {
    match how {
        scaffold::Deleted::Trashed => "已移到废纸篓，可还原",
        scaffold::Deleted::Removed => "系统不支持废纸篓，已永久删除",
    }
}

fn cmd_verify(env: Option<String>, report: Option<&str>, output: Option<String>, json_stream: bool, no_history: bool) -> u8 {
    let fmt = report.unwrap_or("term");
    if !matches!(fmt, "term" | "json") {
        return fatal(format!("不支持的报告格式 {fmt}（verify 可选 term / json）"));
    }
    let root = match project_root_or_err() {
        Ok(r) => r,
        Err(_) => return 2,
    };
    let spec = verify::VerifySpec {
        root,
        env,
        output,
        send_mode: false,
        source: run_source().into(),
        record_history: !no_history,
        cancel: None,
    };
    let is_term = fmt == "term" && !json_stream;
    let outcome = verify::run(&spec, run_hook(json_stream, is_term), |phase| {
        if is_term {
            term::print_verify_stage(phase);
        }
    });
    let out = match outcome {
        Ok(o) => o,
        Err(e) => return fatal(format!("{e:#}")),
    };
    if json_stream {
        return out.exit_code();
    }
    if fmt == "json" {
        println!("{}", report::render_json(&out.report));
    } else {
        term::print_notices(&out.notices);
        term::print_verify_verdict(&out);
    }
    out.exit_code()
}

/// `send --suggest`：真实响应 → 可直接粘贴的 expect / capture（与 Desktop 响应面板「字段」页同一实现）
fn print_suggestions(status: &str, body: &str) {
    let fields = requestbaby_core::suggest::fields(body, 3, 60);
    let (dim, reset) = ("\x1b[2m", "\x1b[0m");
    if fields.is_empty() {
        println!("{dim}── 建议断言：响应体不是 JSON，可断言状态码与整个响应体{reset}");
        println!("  \"expect\": {{\"status\": {status}, \"body\": {{\"$\": {{\"contains\": \"…\"}}}}}}");
        return;
    }
    println!("{dim}── 建议断言：粘贴进用例的 \"expect\": {{\"status\": {status}, \"body\": {{ … }}}}（值每次会变的 id / 时间 / token 用右侧的类型断言）{reset}");
    for f in &fields {
        let alt = f.type_expect().snippet();
        let alt = alt.split_once(": ").map(|(_, m)| m).unwrap_or_default();
        println!("  {:<56} {dim}// 或 {alt}{reset}", format!("{},", f.value_expect().snippet()));
    }
    println!("{dim}── 建议捕获：粘贴进 \"capture\": {{ … }}，后续用例用 {{{{名称}}}} 引用{reset}");
    for f in fields.iter().filter(|f| !matches!(f.value, serde_json::Value::Object(_) | serde_json::Value::Array(_))).take(12) {
        println!("  {},", f.capture(&f.var_name()).snippet());
    }
}

fn cmd_send(case: &str, env: Option<&str>, suggest: bool, save_body: Option<&str>, headed: bool) -> u8 {
    let path = PathBuf::from(case);
    if !path.exists() {
        return err(format!("用例不存在 {case}"));
    }
    let mut spec = session::send_spec(path, case, env.map(String::from), run_source(), None);
    spec.opts.headed = headed;
    let out = match session::run(&spec, None, |_, _, _| {}) {
        Ok(o) => o,
        Err(e) => return err(format!("{e:#}")),
    };
    // 自动补跑的前置用例排在前面：只汇报它们的结论，详情看目标用例
    let Some(res) = out.results.iter().find(|r| !out.prerequisites.contains(&r.rel)) else {
        return err("未发现用例");
    };

    let cyan = "\x1b[36m";
    let green = "\x1b[32m";
    let dim = "\x1b[2m";
    let bold = "\x1b[1m";
    let reset = "\x1b[0m";
    for n in out.notices.iter().filter(|n| n.starts_with("先运行前置用例")) {
        println!("{dim}↪ {n}{reset}");
    }
    for p in out.results.iter().filter(|r| out.prerequisites.contains(&r.rel)) {
        let mark = if p.passed() { format!("{green}✔{reset}") } else { "\x1b[31m✘\x1b[0m".to_string() };
        println!("  {mark} {dim}{} {}{reset}{}", p.status_text(), p.rel, p.error.as_ref().map(|e| format!("  {e}")).unwrap_or_default());
    }
    println!("{bold}{cyan}{} {reset}{bold}{}{reset}", res.method, res.url);
    if let Some(e) = &res.error {
        println!("\x1b[31m✘ {e}{reset}");
    }
    if let Some(resp) = &res.full_response {
        use requestbaby_core::config::Protocol;
        let code = if res.status_is_bad() { "\x1b[31m" } else { green };
        println!("{dim}──{reset} 状态 {code}{bold}{}{reset} {dim}· {}ms · {} bytes{reset}", res.status_text(), resp.time_ms, resp.size);
        let (head_title, body_title) = match resp.protocol {
            Protocol::Http => ("响应头", "响应体"),
            Protocol::Grpc => ("响应元数据", "响应消息"),
            Protocol::Ws if resp.status != 101 => ("握手响应头", "握手响应体"),
            Protocol::Ws => ("握手响应头", "收到的消息"),
            Protocol::Web => ("页面响应头", "结束时的页面"),
            Protocol::Android => ("", "结束时的界面"),
            Protocol::Desktop => ("", "结束时的窗口"),
        };
        // 界面用例：步骤时间线（每一步的截图）+ 运行环境 + 失败现场
        if let Some(ev) = &resp.evidence {
            println!("{dim}──{reset} 步骤 {dim}（截图在 {}）{reset}", ev.dir);
            term::print_ui_steps(ev, "  ");
            if !ev.environment.is_empty() {
                println!("{dim}── 环境 {}{reset}", ev.environment.iter().filter(|(k, _)| k != "User-Agent").map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(" · "));
            }
            for a in &ev.artifacts {
                println!("{dim}── 失败现场 {}：{}/{}{reset}", a.label, ev.dir, a.file);
            }
        }
        if !resp.headers.is_empty() && !resp.protocol.is_ui() {
            println!("{dim}──{reset} {head_title}");
            for (k, v) in &resp.headers {
                println!("  {cyan}{k}{reset}{dim}:{reset} {v}");
            }
        }
        // WebSocket / gRPC 流：按时间顺序的消息（→ 发出 ← 收到）
        if resp.protocol == Protocol::Ws && !resp.frames.is_empty() {
            println!("{dim}──{reset} 对话");
            for f in &resp.frames {
                let arrow = if f.sent { format!("{cyan}→{reset}") } else { format!("{green}←{reset}") };
                let kind = if f.kind == "text" { String::new() } else { format!("{dim}[{}]{reset} ", f.kind) };
                println!("  {arrow} {dim}{:>6}ms{reset} {kind}{}", f.at_ms, f.data);
            }
        }
        println!("{dim}──{reset} {body_title}");
        match &res.raw_body {
            Some(raw) => println!("{dim}<二进制响应 {} 字节；用 --save-body <文件> 保存>{reset}", raw.len()),
            None => println!("{}", pretty_json_colored(&resp.body)),
        }
        if let Some(file) = save_body {
            let bytes: &[u8] = res.raw_body.as_deref().map(|b| b.as_slice()).unwrap_or(resp.body.as_bytes());
            match requestbaby_core::fsx::write(file, bytes) {
                Ok(()) => println!("{green}✔{reset} 响应体已保存 {file}（{} 字节）", bytes.len()),
                Err(e) => eprintln!("\x1b[31m保存失败 {file}: {e}{reset}"),
            }
        }
        if suggest {
            let status = if resp.protocol == requestbaby_core::config::Protocol::Grpc { format!("\"{}\"", res.status_text()) } else { resp.status.to_string() };
            print_suggestions(&status, &resp.body);
        }
    }
    if !res.tests.is_empty() {
        let passed = res.tests.iter().filter(|t| t.pass).count();
        println!("{dim}──{reset} 断言 {green}{passed}{reset}/{dim}{}{reset}", res.tests.len());
        for t in &res.tests {
            let sym = if t.pass { format!("{green}✔{reset}") } else { format!("\x1b[31m✘{reset}") };
            println!("  {sym} {}", t.expr);
        }
    }
    if !res.logs.is_empty() {
        println!("{dim}──{reset} 日志");
        for l in &res.logs {
            println!("  {dim}{l}{reset}");
        }
    }
    if res.passed() { 0 } else { 1 }
}

fn cmd_vars(case: &str, env: Option<&str>, json: bool) -> u8 {
    let path = PathBuf::from(case);
    if !path.exists() {
        return err(format!("用例不存在 {case}"));
    }
    let env = session::resolve_env(std::slice::from_ref(&path), env);
    let p = match inspect::case_vars(&path, &env) {
        Ok(p) => p,
        Err(e) => return err(format!("{e:#}")),
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&p).unwrap_or_default());
        return if p.undefined.is_empty() { 0 } else { 2 };
    }
    let (dim, red, yellow, cyan, reset) = ("\x1b[2m", "\x1b[31m", "\x1b[33m", "\x1b[36m", "\x1b[0m");
    println!("\x1b[1m变量解析\x1b[0m {dim}env={}{reset}", p.env);
    if let Ok(c) = requestbaby_core::config::load_strict::<requestbaby_core::config::TestCase>(&path) {
        let (method, url) = c.summary();
        println!("{dim}请求{reset} {method} {}", p.render_preview(&url));
    }
    for e in &p.entries {
        let mark = if e.used { format!("{cyan}●{reset}") } else { format!("{dim}○{reset}") };
        let over = if e.overridden.is_empty() { String::new() } else { format!(" {dim}(覆盖 {}){reset}", e.overridden.join(" → ")) };
        println!("{mark} {:<20} = {:<32} {dim}← {}{reset}{over}", e.name, e.value, e.source);
    }
    if !p.runtime.is_empty() {
        println!("{dim}── 运行时捕获（脚本写入 vars.x）{reset}");
        for r in &p.runtime {
            let mark = if r.used { format!("{cyan}●{reset}") } else { format!("{dim}○{reset}") };
            println!("{mark} {:<20} {dim}← {}{reset}", r.name, r.writers.join(", "));
        }
    }
    for (name, set) in &p.process_env {
        if *set {
            println!("{cyan}●{reset} %{name:<19} {dim}← 进程环境变量（已设置）{reset}");
        } else {
            println!("{yellow}⚠{reset} %{name:<19} {yellow}进程环境变量未设置{reset}");
        }
    }
    for u in &p.undefined {
        println!("{red}✘ {u} 未定义{reset}");
    }
    if p.undefined.is_empty() { 0 } else { 2 }
}

fn project_root_or_err() -> Result<PathBuf, u8> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    session::project_root(&cwd).ok_or_else(|| err("未找到项目根（requestbaby.json）"))
}

fn cmd_vars_matrix(json: bool) -> u8 {
    let root = match project_root_or_err() {
        Ok(r) => r,
        Err(code) => return code,
    };
    let m = match varstore::matrix(&root) {
        Ok(m) => m,
        Err(e) => return err(format!("{e:#}")),
    };
    let undefined = m.undefined().count();
    if json {
        println!("{}", serde_json::to_string_pretty(&m).unwrap_or_default());
        return if undefined == 0 { 0 } else { 2 };
    }
    let (dim, red, cyan, reset) = ("\x1b[2m", "\x1b[31m", "\x1b[36m", "\x1b[0m");
    let cell = |s: &str| {
        let s: String = s.chars().take(22).collect();
        format!("{s:<24}")
    };
    println!("\x1b[1m变量总览\x1b[0m {dim}优先级：低 → 高（右侧覆盖左侧），运行时捕获最高{reset}");
    print!("{:<20}", "");
    for c in &m.columns {
        print!("{cyan}{}{reset}", cell(&c.label));
    }
    println!("{dim}引用{reset}");
    for r in &m.rows {
        print!("{:<20}", r.name);
        for c in &r.cells {
            match c {
                Some(d) => print!("{}", cell(&d.display(&r.name))),
                None => print!("{dim}{}{reset}", cell("·")),
            }
        }
        let mut note = format!("{} 个用例", r.used_by.len());
        if !r.runtime_writers.is_empty() {
            note.push_str(&format!(" · 运行时捕获 ← {}", r.runtime_writers.join(", ")));
        }
        if r.cells.iter().all(Option::is_none) && r.runtime_writers.is_empty() {
            println!("{red}{note} · 未定义{reset}");
        } else {
            println!("{dim}{note}{reset}");
        }
    }
    if undefined == 0 { 0 } else { 2 }
}

fn cmd_vars_edit(cmd: VarsCmd) -> u8 {
    let root = match project_root_or_err() {
        Ok(r) => r,
        Err(code) => return code,
    };
    let rel = |p: &std::path::Path| p.strip_prefix(&root).unwrap_or(p).display().to_string();
    match cmd {
        VarsCmd::Set { name, value, scope, secret, from_env } => {
            let scope = scope.scope(&root);
            let def = match (from_env, value) {
                (Some(n), _) => VarDef::FromEnv((!n.is_empty()).then_some(n)),
                (None, Some(v)) if secret => VarDef::Secret(v),
                (None, Some(v)) => VarDef::Plain(v),
                (None, None) => return err("缺少值（或使用 --from-env）"),
            };
            match varstore::set(&root, &scope, &name, &def) {
                Ok(file) => {
                    println!("\x1b[32m✔\x1b[0m {name} = {} \x1b[2m← {} · {}\x1b[0m", def.display(&name), scope.label(), rel(&file));
                    0
                }
                Err(e) => err(format!("{e:#}")),
            }
        }
        VarsCmd::Unset { name, scope } => {
            let scope = scope.scope(&root);
            match varstore::unset(&root, &scope, &name) {
                Ok(true) => {
                    println!("\x1b[32m✔\x1b[0m 已从{}删除 {name}", scope.label());
                    0
                }
                Ok(false) => err(format!("{}中没有变量 {name}", scope.label())),
                Err(e) => err(format!("{e:#}")),
            }
        }
    }
}

fn cmd_fmt(paths: &[String], check: bool, fix: bool) -> u8 {
    let paths: Vec<String> = if paths.is_empty() { vec!["collections".into()] } else { paths.to_vec() };
    let pb: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    let report = match fmt::format_paths(&pb, check, fix) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("\x1b[31m错误: {e:#}\x1b[0m");
            return 2;
        }
    };
    for c in &report.changes {
        match c {
            fmt::FmtChange::Case(f) if check => println!("\x1b[33m可精简\x1b[0m {}", f.display()),
            fmt::FmtChange::Case(f) => println!("\x1b[32m已格式化\x1b[0m {}", f.display()),
            fmt::FmtChange::HoistedHeaders(f) => println!("\x1b[32m已上提 headers\x1b[0m {}", f.display()),
            fmt::FmtChange::Upgraded(f, applied) => {
                println!("{} {}", if check { "\x1b[33m可改写\x1b[0m" } else { "\x1b[32m已改写\x1b[0m" }, f.display());
                for a in applied {
                    println!("  \x1b[2m· {a}\x1b[0m");
                }
            }
        }
    }
    if report.case_changes() == 0 {
        println!("\x1b[32m✔ 已是精简形态\x1b[0m");
    }
    if check && report.case_changes() > 0 { 1 } else { 0 }
}

fn cmd_doctor(env: Option<&str>, json: bool) -> u8 {
    let r = doctor::run(std::path::Path::new("."), env);
    if json {
        println!("{}", serde_json::to_string_pretty(&r).unwrap_or_default());
    } else {
        println!("\x1b[1mrequestbaby doctor\x1b[0m");
        for c in &r.checks {
            let mark = match c.level {
                doctor::CheckLevel::Ok => "\x1b[32m✔\x1b[0m",
                doctor::CheckLevel::Warn => "\x1b[33m⚠\x1b[0m",
                doctor::CheckLevel::Error => "\x1b[31m✘\x1b[0m",
            };
            println!("  {mark} {}", c.message);
        }
    }
    if r.ok() { 0 } else { 2 }
}

fn default_env_of(paths: &[PathBuf]) -> Option<String> {
    session::default_env_of(paths)
}

fn cmd_validate(paths: &[String], env: Option<&str>, report: Option<&str>) -> u8 {
    let json = report.map(|r| r == "json").unwrap_or(false);
    let explicit: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    let probe = if explicit.is_empty() { vec![PathBuf::from(".")] } else { explicit.clone() };
    let env = env
        .map(String::from)
        .or_else(|| default_env_of(&probe))
        .unwrap_or_else(|| "dev".into());
    // 不带路径：collections + 本环境会运行的流程引用的目录（与 verify / 桌面端校验面板同一范围）
    let pb = if explicit.is_empty() {
        match session::project_root(std::path::Path::new(".")) {
            Some(root) => validate::project_scope(&root, &env),
            None => vec![PathBuf::from("collections")],
        }
    } else {
        explicit
    };
    match validate::validate_paths(&pb, &env) {
        Ok(issues) => {
            let errors = issues.iter().filter(|i| i.level == "error").count();
            let warns = issues.len() - errors;
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "ok": errors == 0,
                        "errors": errors,
                        "warnings": warns,
                        "issues": issues,
                    })
                );
                return if errors > 0 { 2 } else { 0 };
            }
            for i in &issues {
                let (color, sym) = if i.level == "error" { ("\x1b[31m", "✘") } else { ("\x1b[33m", "⚠") };
                println!("{color}{sym}\x1b[0m {file} [{rule}]: {msg}", sym = sym, color = color, file = i.file, rule = i.rule, msg = i.message);
            }
            if issues.is_empty() {
                println!("\x1b[32m✔ 全部通过\x1b[0m");
                0
            } else {
                println!("\n{} 错误, {} 警告", errors, warns);
                let fixable = issues.iter().filter(|i| i.fixable).count();
                if fixable > 0 {
                    println!("\x1b[2m{fixable} 条写法建议可自动改写（语义不变）：requestbaby fmt --fix [目录]\x1b[0m");
                }
                if errors > 0 { 2 } else { 0 }
            }
        }
        Err(e) => {
            eprintln!("校验失败: {e:#}");
            2
        }
    }
}

fn cmd_list(paths: &[String]) -> u8 {
    let paths: Vec<String> = if paths.is_empty() { vec!["collections".into()] } else { paths.to_vec() };
    let pb: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    match runner::discover::discover(&pb) {
        Ok(cases) => {
            for dc in &cases {
                let chain: Vec<String> = dc.chain.iter().map(|c| c.name.clone()).collect();
                println!(
                    "\x1b[36m{}\x1b[0m  \x1b[2m[{}]\x1b[0m",
                    dc.rel,
                    chain.join(" / ")
                );
            }
            println!("\n共 {} 个用例", cases.len());
            0
        }
        Err(e) => {
            eprintln!("{e:#}");
            1
        }
    }
}

fn new_case_from_curl(dir: &str, name: Option<&str>, curl: &str, env: Option<&str>) -> anyhow::Result<PathBuf> {
    let curl = if curl == "-" {
        let mut buf = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
        buf
    } else {
        curl.to_string()
    };
    let dir = PathBuf::from(dir);
    let base_url = session::project_root(&dir).and_then(|root| {
        let env = session::resolve_env(std::slice::from_ref(&root), env);
        config::load_env(&root, &env).ok().flatten()?.vars.get("baseUrl").map(|v| v.value.clone())
    });
    let cwd = std::env::current_dir().ok();
    scaffold::new_case_from_curl(&dir, name, &curl, base_url.as_deref(), cwd.as_deref())
}

fn cmd_import(spec: Option<&str>, curl: Option<&str>, out: Option<&str>, env: Option<&str>) -> u8 {
    let out_dir = std::path::PathBuf::from(out.unwrap_or("imported"));
    let env_name = env.unwrap_or("dev");
    // curl 命令模式：单用例导入
    if let Some(cmdline) = curl {
        return match importer::import_curl(cmdline) {
            Ok(case) => {
                let dir = out_dir.join("collections");
                let _ = std::fs::create_dir_all(&dir);
                let file = dir.join("01-imported-from-curl.json");
                requestbaby_core::fsx::write(&file, serde_json::to_vec_pretty(&case).unwrap_or_default()).ok();
                println!("\x1b[32m✔\x1b[0m curl 命令已导入: {}", file.display());
                0
            }
            Err(e) => {
                eprintln!("\x1b[31m导入失败: {e:#}\x1b[0m");
                2
            }
        };
    }
    let Some(spec) = spec else {
        eprintln!("\x1b[31m错误: 需要 spec 文件或 --curl 命令\x1b[0m");
        return 2;
    };
    match importer::import(std::path::Path::new(spec), &out_dir, env_name) {
        Ok(sum) => {
            println!(
                "\x1b[32m✔\x1b[0m 导入完成: {} 个用例 / {} 个集合 → {}（baseUrl: {}）",
                sum.cases,
                sum.collections,
                sum.out_dir.display(),
                sum.base_url
            );
            for w in &sum.warnings {
                println!("\x1b[33m⚠\x1b[0m {w}");
            }
            println!("\n下一步: cd {} && requestbaby run collections --dry-run", sum.out_dir.display());
            0
        }
        Err(e) => {
            eprintln!("\x1b[31m导入失败: {e:#}\x1b[0m");
            1
        }
    }
}

fn cmd_watch(paths: &[String], env: Option<&str>, tags: Vec<String>, name: Option<&str>) -> u8 {
    let paths: Vec<String> = if paths.is_empty() { vec!["collections".into()] } else { paths.to_vec() };
    let pb: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    // watch 目标 = 用例 + 项目/集合/环境配置
    let mut watch_files: Vec<PathBuf> = pb.clone();
    if let Ok(cases) = runner::discover::discover(&pb) {
        watch_files.extend(cases.iter().map(|c| c.path.clone()));
    }
    if let Some(root) = runner::discover::find_project_root(&pb[0]) {
        watch_files.push(root.join("requestbaby.json"));
        let envs = root.join("envs");
        if let Ok(rd) = std::fs::read_dir(&envs) {
            for e in rd.flatten() {
                watch_files.push(e.path());
            }
        }
        if let Ok(rd) = std::fs::read_dir(root.join("collections")) {
            for e in rd.flatten() {
                if e.path().join("collection.json").exists() {
                    watch_files.push(e.path().join("collection.json"));
                }
            }
        }
    }

    eprintln!("\x1b[2mwatching {} 个文件，变更自动重跑（Ctrl-C 退出）\x1b[0m", watch_files.len());
    let mut args: Vec<String> = vec!["run".into()];
    args.extend(pb.iter().map(|p| p.display().to_string()));
    if let Some(e) = env {
        args.push("-e".into());
        args.push(e.into());
    }
    for t in tags {
        args.push("--tag".into());
        args.push(t);
    }
    if let Some(n) = name {
        args.push("--name".into());
        args.push(n.into());
    }
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("requestbaby"));
    loop {
        print!("\x1b[2J\x1b[H");
        let _ = std::process::Command::new(&exe).args(&args).status();
        eprintln!("\n\x1b[2m── 已完成，等待变更…\x1b[0m");
        // 事件驱动等待：任一 watch 文件变更/新建即触发重跑
        let (tx, rx) = std::sync::mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |res: Result<notify::Event, _>| {
            if res.is_ok() {
                let _ = tx.send(());
            }
        })
        .expect("文件监听初始化失败");
        use notify::Watcher;
        for f in &watch_files {
            let _ = watcher.watch(f, notify::RecursiveMode::NonRecursive);
        }
        // 父目录监听捕获新建文件（新增用例）
        for f in watch_files.iter().filter_map(|p| p.parent()).collect::<std::collections::BTreeSet<_>>() {
            let _ = watcher.watch(f, notify::RecursiveMode::NonRecursive);
        }
        let _ = rx.recv();
    }
}

/// 桌面端可执行文件：与 CLI 同目录优先，其次 PATH
/// 查找桌面端：同目录（开发构建）→ macOS 已安装的 requestbaby.app → PATH
fn desktop_exe() -> Option<PathBuf> {
    let name = if cfg!(windows) { "requestbaby-desktop.exe" } else { "requestbaby-desktop" };
    if let Some(sibling) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join(name))) {
        if sibling.is_file() {
            return Some(sibling);
        }
    }
    if cfg!(target_os = "macos") {
        let bundle = PathBuf::from("Applications/requestbaby.app/Contents/MacOS").join(name);
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let app = [Some(PathBuf::from("/")), home].into_iter().flatten().map(|base| base.join(&bundle)).find(|p| p.is_file());
        if app.is_some() {
            return app;
        }
    }
    std::env::var_os("PATH").and_then(|paths| std::env::split_paths(&paths).map(|d| d.join(name)).find(|p| p.is_file()))
}

fn cmd_open(path: Option<&str>) -> u8 {
    let dir = PathBuf::from(path.unwrap_or("."));
    let dir = std::fs::canonicalize(&dir).unwrap_or(dir);
    let root = session::project_root(&dir).unwrap_or(dir);
    let Some(exe) = desktop_exe() else {
        eprintln!(
            "\x1b[33m未找到 requestbaby-desktop。\x1b[0m macOS：安装 requestbaby.app 到 /Applications（scripts/build-mac-release.sh）；其他平台：cargo install --path crates/requestbaby-desktop"
        );
        return 1;
    };
    match std::process::Command::new(&exe).arg(&root).spawn() {
        Ok(_) => {
            println!("\x1b[32m✔\x1b[0m 已在桌面端打开 {}", root.display());
            0
        }
        Err(e) => {
            eprintln!("\x1b[31m启动桌面端失败: {e}\x1b[0m");
            1
        }
    }
}

fn cmd_export(id: Option<String>, env: Option<String>, collection: Vec<String>, format: &str, output: Option<String>) -> u8 {
    use requestbaby_core::export;
    let root = match project_root_or_err() {
        Ok(r) => r,
        Err(code) => return code,
    };
    let format = match export::Format::parse(format) {
        Ok(f) => f,
        Err(e) => return fatal(e),
    };
    if id.is_some() && !collection.is_empty() {
        return fatal("运行 ID 与 --collection 不能同时指定：--collection 取的是该集合每个用例的最新结果");
    }
    let collection: Vec<String> = collection.iter().map(|c| export::normalize_collection(&root, c)).collect();
    let scope = match id {
        Some(id) => export::Scope::Run(id),
        None => export::Scope::Latest { env: session::resolve_env(std::slice::from_ref(&root), env.as_deref()), collections: collection.clone() },
    };
    let report = match export::collect(&root, &scope) {
        Ok(r) => r,
        Err(e) => return fatal(format!("{e:#}")),
    };
    let name = export::file_name(&root, &report, format);
    let path = match output.map(PathBuf::from) {
        Some(p) if p.is_dir() => p.join(&name),
        Some(p) => p,
        None => PathBuf::from(&name),
    };
    if let Err(e) = requestbaby_core::fsx::write(&path, export::render(&report, &root, format)) {
        return err(format!("写入 {} 失败: {e:#}", path.display()));
    }
    let overview = export::Overview::of(&report, &root, &collection);
    println!("\x1b[32m✔\x1b[0m 已导出 {}（{}）", path.display(), overview.headline());
    for note in overview.notes() {
        println!("\x1b[33m!\x1b[0m {note}");
    }
    0
}

fn cmd_history(limit: usize, json: bool) -> u8 {
    let Some(root) = session::project_root(std::path::Path::new(".")) else {
        eprintln!("\x1b[31m错误: 未找到项目根（requestbaby.json）\x1b[0m");
        return 1;
    };
    let entries: Vec<history::RunEntry> = history::list(&root).into_iter().take(limit).collect();
    if json {
        let reports: Vec<&report::RunReport> = entries.iter().map(|e| &e.report).collect();
        println!("{}", serde_json::to_string_pretty(&reports).unwrap_or_default());
        return 0;
    }
    if entries.is_empty() {
        println!("\x1b[2m暂无运行历史（{}）\x1b[0m", history::runs_dir(&root).display());
        return 0;
    }
    for e in &entries {
        let r = &e.report;
        let m = r.meta.clone().unwrap_or_default();
        let mark = if r.failed > 0 { "\x1b[31m✘\x1b[0m" } else { "\x1b[32m✔\x1b[0m" };
        println!(
            "{mark} \x1b[2m{}\x1b[0m {:<8} env={:<6} {}/{} passed  \x1b[2m{}\x1b[0m",
            m.id, m.source, m.env, r.passed, r.total, m.command
        );
    }
    0
}

fn cmd_history_clean(action: HistoryCmd) -> u8 {
    let root = match project_root_or_err() {
        Ok(r) => r,
        Err(_) => return 2,
    };
    let r: anyhow::Result<String> = (|| match action {
        HistoryCmd::Rm { ids } => {
            for id in &ids {
                history::delete(&root, id)?;
            }
            Ok(format!("已删除 {} 条运行记录", ids.len()))
        }
        HistoryCmd::Clear { all: false, keep: None, older_than: None } => {
            anyhow::bail!("请指定清理范围：--all（全部）/ --keep <N> / --older-than <7d>")
        }
        HistoryCmd::Clear { keep, older_than, .. } => {
            let older_than_ms = older_than.as_deref().map(history::parse_duration_ms).transpose()?;
            let n = history::clean(&root, history::Clean { keep, older_than_ms })?;
            Ok(format!("已清理 {n} 条运行记录（剩余 {} 条）", history::list_paths(&root).len()))
        }
    })();
    match r {
        Ok(msg) => {
            println!("\x1b[32m✔\x1b[0m {msg}");
            0
        }
        Err(e) => fatal(format!("{e:#}")),
    }
}

/// JSON pretty + 轻量着色；非 JSON 原样返回
fn pretty_json_colored(body: &str) -> String {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return body.to_string();
    };
    let pretty = serde_json::to_string_pretty(&v).unwrap_or_default();
    // 轻量着色：key 青、字符串绿、数字黄
    let mut out = String::with_capacity(pretty.len() * 2);
    let mut in_str = false;
    let chars: Vec<char> = pretty.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        match c {
            '"' => {
                in_str = !in_str;
                if in_str {
                    // key 判定：向后看下一个非空白字符是否 ':'
                    let mut j = i + 1;
                    while j < chars.len() && chars[j] != '"' {
                        j += 1;
                    }
                    // 找配对引号后的第一个非空白
                    let mut k = j + 1;
                    while k < chars.len() && chars[k].is_whitespace() {
                        k += 1;
                    }
                    let is_key = k < chars.len() && chars[k] == ':';
                    out.push_str(if is_key { "\x1b[36m\"" } else { "\x1b[32m\"" });
                } else {
                    out.push('"');
                    out.push_str("\x1b[0m");
                }
            }
            '0'..='9' if !in_str => {
                out.push_str("\x1b[33m");
                out.push(c);
                // 数字结束判断交给下一个非数字字符重置
                let next = chars.get(i + 1);
                if !next.map(|n| n.is_ascii_digit()).unwrap_or(false) {
                    out.push_str("\x1b[0m");
                }
            }
            _ => out.push(c),
        }
    }
    out
}
