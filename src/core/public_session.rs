use std::collections::HashMap;

use tokio::sync::RwLock;

const SESSION_TTL: f64 = 600.0;

pub struct SessionStore<T> {
    inner: RwLock<HashMap<String, (T, f64)>>,
}

fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

impl<T: Clone + Send + Sync + 'static> SessionStore<T> {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
        }
    }

    pub async fn create(&self, data: T) -> String {
        let task_id = uuid::Uuid::new_v4().to_string();
        let mut map = self.inner.write().await;
        Self::purge_expired(&mut map);
        map.insert(task_id.clone(), (data, now_secs()));
        task_id
    }

    pub async fn get(&self, task_id: &str) -> Option<T> {
        let mut map = self.inner.write().await;
        Self::purge_expired(&mut map);
        map.get(task_id).map(|(data, _)| data.clone())
    }

    #[allow(dead_code)]
    pub async fn remove(&self, task_id: &str) {
        let mut map = self.inner.write().await;
        map.remove(task_id);
    }

    pub async fn remove_many(&self, task_ids: &[String]) -> usize {
        let mut map = self.inner.write().await;
        let mut removed = 0;
        for id in task_ids {
            if map.remove(id).is_some() {
                removed += 1;
            }
        }
        removed
    }

    fn purge_expired(map: &mut HashMap<String, (T, f64)>) {
        let cutoff = now_secs() - SESSION_TTL;
        map.retain(|_, (_, ts)| *ts > cutoff);
    }
}
