//! requestbaby 核心库：CLI 与 Desktop 共享同一套数据体系与协议。
//!
//! 规则：本 crate 不直接写 stdout/stderr、不输出 ANSI 颜色；
//! 一切运行期信息通过 [`runner::RunEvent`] 事件流交给上层渲染。
//! 两端的每个能力（运行、工作流、单发、校验、格式化、脚手架、自检、变量预览）都只在这里实现一次。
pub mod config;
pub mod doctor;
pub mod engine;
pub mod fmt;
pub mod history;
pub mod importer;
pub mod inspect;
pub mod report;
pub mod runner;
pub mod scaffold;
pub mod script;
pub mod session;
pub mod validate;
pub mod vars;
pub mod workflow;
