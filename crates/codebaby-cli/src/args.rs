//! 命令行参数定义（clap）

use super::*;

#[derive(Parser)]
#[command(
    name = "codebaby",
    version,
    about = "Independent acceptance layer for AI-delivered work: JSON test cases for HTTP / gRPC / WebSocket APIs and Web / Android UIs, with step-by-step screenshot evidence",
    after_help = "官网与下载：https://apps.zhizuzhefu.com/codebaby/　反馈：https://github.com/zhizuzhefu/codebaby/issues",
    long_about = "codebaby — AI 交付的独立验收层，辅助人对 AI 交付的成果验收：在接口边界核对行为，一个 JSON 文件一个用例。\nAI / LLM 对接：codebaby guide（用例格式 + 调度协议）；配置契约：codebaby schema --all。"
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) cmd: Cmd,
    /// 本次经这个代理访问（socks5:// / socks5h:// / http://，覆盖 codebaby.json 的 proxy）
    #[arg(long, global = true, value_name = "URL", conflicts_with = "no_proxy")]
    pub(crate) proxy: Option<String>,
    /// 本次不使用代理：全部直连（也不读 http_proxy 等环境变量）
    #[arg(long, global = true)]
    pub(crate) no_proxy: bool,
}

#[derive(Subcommand)]
pub(crate) enum Cmd {
    /// 初始化项目脚手架
    Init {
        /// 目标目录（缺省当前目录）
        path: Option<String>,
        /// 被测服务的地址，写进 dev 环境的 baseUrl（缺省 http://localhost:8080，之后可用 `vars set baseUrl` 改）
        #[arg(long, value_name = "URL")]
        base_url: Option<String>,
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
        /// 与 --init 一起用：旧版 codebaby 生成的 AGENTS.md 更新为当前版本（你自己写的不覆盖）
        #[arg(long, requires = "init")]
        update: bool,
    },
    /// gRPC：列出服务的方法、查看请求消息骨架（来源：codebaby.json 的 grpc.protos，未配置则服务端反射）
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
    /// 定义在 codebaby.json 的 workflows 字段；多阶段编排，变量跨步骤自动传递
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
        /// 报告格式：term / json / html / junit（JUnit XML，输出到标准输出，CI 的测试面板直接读）
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
    /// 校验、执行并核对必验用例（0 通过 / 1 用例失败 / 2 验收不完整）
    Verify {
        #[arg(short, long)]
        env: Option<String>,
        /// 只执行指定流程及其依赖，不另外扫描运行 collections/
        #[arg(long)]
        workflow: Option<String>,
        /// 必验用例的项目相对 JSON 路径，可重复指定；省略时要求计划中的所有用例
        #[arg(long = "require-case", value_name = "FILE")]
        required_cases: Vec<String>,
        /// 只查看计划，不发请求、不执行脚本、不写运行历史
        #[arg(long, conflicts_with_all = ["json_stream", "output"])]
        plan: bool,
        /// 报告格式：term（默认）/ json（合并报告：steps = 阶段，issues = 校验问题，blocked = 无法完成的原因）/ junit（JUnit XML）
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
        /// 环境名（缺省用 codebaby.json 的 defaultEnv，再缺省 dev）
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
        /// 报告格式：term（默认）/ json / html（写 report.html）/ junit（JUnit XML，输出到标准输出，如 `> junit.xml`）
        #[arg(long)]
        report: Option<String>,
        /// 逐行输出运行事件（NDJSON，与 Desktop 同一事件协议），替代常规报告
        #[arg(long)]
        json_stream: bool,
        /// 不写入运行历史（.codebaby/runs/）
        #[arg(long)]
        no_history: bool,
        /// Web 用例显示浏览器窗口（缺省无界面运行；也可设 CODEBABY_HEADED=1）
        #[arg(long)]
        headed: bool,
    },
    /// 导出验收结果交给别人：缺省 = 全部用例的最新结果，一份 HTML（截图已嵌入，直接用浏览器打开）；也可导出某一次运行（与桌面端「导出结果」同一实现）
    Export {
        /// 运行 ID：只导出这一次运行（ID 见 codebaby history）；缺省导出环境下每个用例的最新结果
        id: Option<String>,
        /// 环境名（缺省用 codebaby.json 的 defaultEnv，再缺省 dev）；指定运行 ID 时忽略
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
    /// 在桌面端打开项目（需安装 codebaby-desktop）
    Open {
        /// 项目目录（缺省当前目录）
        path: Option<String>,
    },
    /// 运行历史（CLI / Desktop 共享 .codebaby/runs/）：查看；rm / clear 清理（与 Desktop 历史面板同一实现）
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
pub(crate) enum HistoryCmd {
    /// 删除指定的运行记录（ID 见 codebaby history）
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
pub(crate) enum WebCmd {
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
pub(crate) enum ProxyCmd {
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
pub(crate) enum DesktopCmd {
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
pub(crate) enum AndroidCmd {
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
pub(crate) enum NewCmd {
    /// 在集合目录下新建用例（自动编号 NN-<名称>.json；同 `case new`）
    Case(NewCaseArgs),
    /// 新建集合目录（含 collection.json；同 `collection new`）
    Collection(NewCollectionArgs),
    /// 新建环境 envs/<name>.json（同 `env new`）
    Env(NewEnvArgs),
}

#[derive(Args)]
pub(crate) struct NewCaseArgs {
    /// 集合目录
    pub(crate) dir: String,
    /// 用例名（--curl 时可省略：取「方法 + 路径末段」）
    pub(crate) name: Option<String>,
    #[arg(short, long, default_value = "GET")]
    pub(crate) method: String,
    #[arg(short, long, default_value = "")]
    pub(crate) url: String,
    /// 从 curl 命令生成（浏览器「Copy as cURL」原样粘贴；`-` 从标准输入读取）。
    /// URL 以当前环境 baseUrl 开头时自动改写为 {{baseUrl}}
    #[arg(long, conflicts_with_all = ["method", "url"])]
    pub(crate) curl: Option<String>,
    /// 新建 gRPC 用例：方法名 包.服务/方法（能连上服务或配置了 .proto 时自动填好请求消息骨架）
    #[arg(long, value_name = "METHOD", conflicts_with_all = ["method", "url", "curl", "ws"])]
    pub(crate) grpc: Option<String>,
    /// 与 --grpc 一起：服务地址（缺省 {{grpcTarget}}）
    #[arg(long, requires = "grpc")]
    pub(crate) target: Option<String>,
    /// 新建 WebSocket 用例：连接地址（缺省 {{wsBase}}/）
    #[arg(long, value_name = "URL", num_args = 0..=1, default_missing_value = "", conflicts_with_all = ["method", "url", "curl"])]
    pub(crate) ws: Option<String>,
    /// 新建 Web 界面用例：要打开的页面（缺省 {{webBase}}/）
    #[arg(long, value_name = "URL", num_args = 0..=1, default_missing_value = "", conflicts_with_all = ["method", "url", "curl", "grpc", "ws"])]
    pub(crate) web: Option<String>,
    /// 新建 Android 界面用例：应用包名
    #[arg(long, value_name = "PACKAGE", conflicts_with_all = ["method", "url", "curl", "grpc", "ws", "web"])]
    pub(crate) android: Option<String>,
    /// 新建 Mac 桌面应用用例：应用名称（如 TextEdit）或 bundle ID
    #[arg(long, value_name = "APP", conflicts_with_all = ["method", "url", "curl", "grpc", "ws", "web", "android"])]
    pub(crate) desktop: Option<String>,
    /// 改写 baseUrl 时参考的环境（缺省 defaultEnv）
    #[arg(short, long)]
    pub(crate) env: Option<String>,
}

#[derive(Args)]
pub(crate) struct NewCollectionArgs {
    /// 父目录
    pub(crate) parent: String,
    pub(crate) name: String,
}

#[derive(Args)]
pub(crate) struct NewEnvArgs {
    pub(crate) name: String,
    /// 复制已有环境的变量（不含本机覆盖）
    #[arg(long)]
    pub(crate) from: Option<String>,
}

#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum CaseCmd {
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
pub(crate) enum CollectionCmd {
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
pub(crate) enum EnvCmd {
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
    /// 设为默认环境（codebaby.json 的 defaultEnv）
    Default { name: String },
}

#[derive(Subcommand)]
pub(crate) enum WorkflowCmd {
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
pub(crate) enum GrpcCmd {
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
pub(crate) enum NeedsCmd {
    /// 增加依赖的流程
    Add { need: String },
    /// 去掉依赖的流程
    Rm { need: String },
}

#[derive(Subcommand)]
pub(crate) enum StepCmd {
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
pub(crate) enum VarsCmd {
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
pub(crate) struct ScopeArgs {
    /// 环境 envs/<ENV>.json
    #[arg(short, long)]
    pub(crate) env: Option<String>,
    /// 本机覆盖 envs/<ENV>.local.json：密钥 / 本机端口，自动加入 .gitignore，不入库
    #[arg(long, value_name = "ENV")]
    pub(crate) local: Option<String>,
    /// 项目 codebaby.json（所有环境共享）
    #[arg(long)]
    pub(crate) project: bool,
    /// 集合目录（写入 <DIR>/collection.json）
    #[arg(long, value_name = "DIR")]
    pub(crate) collection: Option<String>,
}

impl ScopeArgs {
    pub(crate) fn scope(&self, root: &std::path::Path) -> VarScope {
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
