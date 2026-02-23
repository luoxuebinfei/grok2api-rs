use std::collections::HashMap;
use std::sync::Arc;

use once_cell::sync::Lazy;
use serde_json::{Value as JsonValue, json};
use tokio::sync::{Mutex, RwLock, mpsc};
use uuid::Uuid;

#[derive(Debug)]
#[allow(dead_code)]
pub struct BatchTask {
    pub id: String,
    pub total: usize,
    pub processed: usize,
    pub ok: usize,
    pub fail: usize,
    pub status: String,
    pub warning: Option<String>,
    pub result: Option<JsonValue>,
    pub error: Option<String>,
    pub created_at: f64,
    pub cancelled: bool,
    final_event: Option<JsonValue>,
    queues: Vec<mpsc::Sender<JsonValue>>,
}

impl BatchTask {
    pub fn new(total: usize) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            total,
            processed: 0,
            ok: 0,
            fail: 0,
            status: "running".to_string(),
            warning: None,
            result: None,
            error: None,
            created_at: chrono::Utc::now().timestamp_millis() as f64 / 1000.0,
            cancelled: false,
            final_event: None,
            queues: Vec::new(),
        }
    }

    pub fn snapshot(&self) -> JsonValue {
        json!({
            "task_id": self.id,
            "status": self.status,
            "total": self.total,
            "processed": self.processed,
            "ok": self.ok,
            "fail": self.fail,
            "warning": self.warning,
        })
    }

    pub fn attach(&mut self) -> mpsc::Receiver<JsonValue> {
        let (tx, rx) = mpsc::channel(200);
        self.queues.push(tx);
        rx
    }

    fn publish(&mut self, event: JsonValue) {
        self.queues.retain(|tx| tx.try_send(event.clone()).is_ok());
    }

    pub fn record(
        &mut self,
        ok: bool,
        item: Option<JsonValue>,
        detail: Option<JsonValue>,
        error: Option<String>,
    ) {
        self.processed += 1;
        if ok {
            self.ok += 1;
        } else {
            self.fail += 1;
        }
        let mut event = json!({
            "type": "progress",
            "task_id": self.id,
            "total": self.total,
            "processed": self.processed,
            "ok": self.ok,
            "fail": self.fail,
        });
        if let Some(item) = item {
            event["item"] = item;
        }
        if let Some(detail) = detail {
            event["detail"] = detail;
        }
        if let Some(error) = error {
            event["error"] = JsonValue::String(error);
        }
        self.publish(event);
    }

    pub fn finish(&mut self, result: JsonValue, warning: Option<String>) {
        self.status = "done".to_string();
        self.result = Some(result.clone());
        self.warning = warning.clone();
        let event = json!({
            "type": "done",
            "task_id": self.id,
            "total": self.total,
            "processed": self.processed,
            "ok": self.ok,
            "fail": self.fail,
            "warning": warning,
            "result": result,
        });
        self.final_event = Some(event.clone());
        self.publish(event);
    }

    #[allow(dead_code)]
    pub fn fail_task(&mut self, error: String) {
        self.status = "error".to_string();
        self.error = Some(error.clone());
        let event = json!({
            "type": "error",
            "task_id": self.id,
            "total": self.total,
            "processed": self.processed,
            "ok": self.ok,
            "fail": self.fail,
            "error": error,
        });
        self.final_event = Some(event.clone());
        self.publish(event);
    }

    pub fn cancel(&mut self) {
        self.cancelled = true;
    }

    pub fn finish_cancelled(&mut self) {
        self.status = "cancelled".to_string();
        let event = json!({
            "type": "cancelled",
            "task_id": self.id,
            "total": self.total,
            "processed": self.processed,
            "ok": self.ok,
            "fail": self.fail,
        });
        self.final_event = Some(event.clone());
        self.publish(event);
    }

    pub fn final_event(&self) -> Option<JsonValue> {
        self.final_event.clone()
    }
}

static TASKS: Lazy<RwLock<HashMap<String, Arc<Mutex<BatchTask>>>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

pub async fn create_task(total: usize) -> Arc<Mutex<BatchTask>> {
    ensure_cleanup_started();
    let task = Arc::new(Mutex::new(BatchTask::new(total)));
    let id = task.lock().await.id.clone();
    TASKS.write().await.insert(id, task.clone());
    task
}

pub async fn get_task(task_id: &str) -> Option<Arc<Mutex<BatchTask>>> {
    TASKS.read().await.get(task_id).cloned()
}

pub async fn delete_task(task_id: &str) {
    TASKS.write().await.remove(task_id);
}

pub async fn expire_task(task_id: String, delay: u64) {
    tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
    delete_task(&task_id).await;
}

/// 后台定时清理过期 BatchTask（已完成/失败/取消且超过 TTL 的任务）
static CLEANUP_STARTED: std::sync::Once = std::sync::Once::new();

const TASK_TTL_SECS: f64 = 1800.0; // 30 分钟
const CLEANUP_INTERVAL_SECS: u64 = 300; // 5 分钟

pub fn ensure_cleanup_started() {
    CLEANUP_STARTED.call_once(|| {
        tokio::spawn(async {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(CLEANUP_INTERVAL_SECS)).await;
                cleanup_expired_tasks().await;
            }
        });
    });
}

async fn cleanup_expired_tasks() {
    let now = chrono::Utc::now().timestamp_millis() as f64 / 1000.0;
    let mut to_remove = Vec::new();

    {
        let tasks = TASKS.read().await;
        for (id, task_arc) in tasks.iter() {
            let task = task_arc.lock().await;
            let is_terminal = task.status == "done"
                || task.status == "error"
                || task.status == "cancelled";
            if is_terminal && (now - task.created_at) > TASK_TTL_SECS {
                to_remove.push(id.clone());
            }
            // 超长时间运行的任务也清理（可能异常挂起）
            if task.status == "running" && (now - task.created_at) > TASK_TTL_SECS * 2.0 {
                to_remove.push(id.clone());
            }
        }
    }

    if !to_remove.is_empty() {
        let count = to_remove.len();
        let mut tasks = TASKS.write().await;
        for id in &to_remove {
            tasks.remove(id);
        }
        tracing::info!("BatchTask 自动清理: 移除 {count} 个过期任务");
    }
}
