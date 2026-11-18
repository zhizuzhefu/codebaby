//! 运行期事件协议：CLI（`run --json-stream` 逐行 NDJSON）与 Desktop（进程内 channel）共用。
//!
//! 事件序列：`runStarted` → [`stepStarted`] (`caseStarted` → `log`* → `caseFinished`)* [`stepFinished`] … → `notice`* → `runFinished`
//! （`stepStarted` / `stepFinished` 仅流程 / verify 运行出现）
use crate::report::{CaseReport, RunReport};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "camelCase")]
pub enum RunEvent {
    #[serde(rename_all = "camelCase")]
    RunStarted { id: String, env: String, source: String, planned: usize },
    /// 流程步骤开始（index 从 0 起）
    #[serde(rename_all = "camelCase")]
    StepStarted { index: usize, total: usize, label: String, run: String, planned: usize },
    #[serde(rename_all = "camelCase")]
    StepFinished { index: usize, passed: usize, failed: usize, skipped: usize },
    #[serde(rename_all = "camelCase")]
    CaseStarted { file: String, name: String },
    #[serde(rename_all = "camelCase")]
    Log { file: String, line: String },
    #[serde(rename_all = "camelCase")]
    CaseFinished { result: CaseReport },
    /// 非用例级提示（teardown 失败等）
    #[serde(rename_all = "camelCase")]
    Notice { message: String },
    #[serde(rename_all = "camelCase")]
    RunFinished { report: RunReport },
}

/// 事件回调（跨线程：并行模式下由工作线程触发）
pub type EventHook = Arc<dyn Fn(&RunEvent) + Send + Sync>;

/// 协作式取消：在用例/数据行边界检查，进行中的单个请求不打断
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}
