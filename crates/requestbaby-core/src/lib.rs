//! requestbaby 核心库：CLI 与 Desktop 共享同一套数据体系与协议。
//!
//! 规则：本 crate 不直接写 stdout/stderr、不输出 ANSI 颜色；
//! 一切运行期信息通过 [`runner::RunEvent`] 事件流交给上层渲染。
//! 两端的每个能力（运行（HTTP / gRPC / WebSocket）、流程、单发、校验、格式化、脚手架、自检、变量预览、完整验证、AI 对接文档）都只在这里实现一次。
pub mod config;
pub mod cookies;
pub mod doctor;
pub mod envs;
pub mod expect;
pub mod engine;
pub mod fmt;
pub mod fsx;
pub mod guide;
pub mod history;
pub mod importer;
pub mod inspect;
pub mod protocols;
pub mod proxy_setting;
pub mod report;
pub mod runner;
pub mod scaffold;
pub mod script;
pub mod session;
pub mod suggest;
pub mod teach;
pub mod update;
pub mod validate;
pub mod vars;
pub mod varstore;
pub mod verify;
pub mod workflow;
