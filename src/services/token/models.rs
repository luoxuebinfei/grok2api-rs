use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TokenStatus {
    Active,
    Disabled,
    Expired,
    Cooling,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EffortType {
    Low,
    High,
}

pub fn effort_cost(effort: &EffortType) -> i32 {
    match effort {
        EffortType::Low => 1,
        EffortType::High => 4,
    }
}

pub const DEFAULT_QUOTA: i32 = 80;
#[allow(dead_code)]
pub const FAIL_THRESHOLD: i32 = 5;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenInfo {
    pub token: String,
    pub status: TokenStatus,
    pub quota: i32,

    pub created_at: i64,
    pub last_used_at: Option<i64>,
    pub use_count: i32,

    pub fail_count: i32,
    pub last_fail_at: Option<i64>,
    pub last_fail_reason: Option<String>,

    pub last_sync_at: Option<i64>,

    pub tags: Vec<String>,
    pub note: String,
    pub last_asset_clear_at: Option<i64>,
}

impl TokenInfo {
    #[allow(dead_code)]
    pub fn new(token: String) -> Self {
        Self {
            token,
            status: TokenStatus::Active,
            quota: DEFAULT_QUOTA,
            created_at: chrono::Utc::now().timestamp_millis(),
            last_used_at: None,
            use_count: 0,
            fail_count: 0,
            last_fail_at: None,
            last_fail_reason: None,
            last_sync_at: None,
            tags: Vec::new(),
            note: String::new(),
            last_asset_clear_at: None,
        }
    }

    #[allow(dead_code)]
    pub fn is_available(&self) -> bool {
        self.status == TokenStatus::Active && self.quota > 0
    }

    pub fn consume(&mut self, effort: &EffortType) -> i32 {
        let cost = effort_cost(effort);
        let actual = std::cmp::min(cost, self.quota);
        self.last_used_at = Some(chrono::Utc::now().timestamp_millis());
        self.use_count += actual;
        self.quota = (self.quota - actual).max(0);
        self.fail_count = 0;
        self.last_fail_reason = None;
        if self.quota == 0 {
            self.status = TokenStatus::Cooling;
        } else if matches!(self.status, TokenStatus::Cooling | TokenStatus::Expired) {
            self.status = TokenStatus::Active;
        }
        actual
    }

    pub fn update_quota(&mut self, new_quota: i32) {
        self.quota = new_quota.max(0);
        if self.quota == 0 {
            self.status = TokenStatus::Cooling;
        } else if matches!(self.status, TokenStatus::Cooling | TokenStatus::Expired) {
            self.status = TokenStatus::Active;
        }
    }

    #[allow(dead_code)]
    pub fn reset(&mut self) {
        self.quota = DEFAULT_QUOTA;
        self.status = TokenStatus::Active;
        self.fail_count = 0;
        self.last_fail_reason = None;
    }

    #[allow(dead_code)]
    pub fn record_fail(&mut self, status_code: u16, reason: &str) {
        if status_code != 401 {
            return;
        }
        self.fail_count += 1;
        self.last_fail_at = Some(chrono::Utc::now().timestamp_millis());
        self.last_fail_reason = Some(reason.to_string());
        if self.fail_count >= FAIL_THRESHOLD {
            self.status = TokenStatus::Expired;
        }
    }

    pub fn record_success(&mut self, is_usage: bool) {
        self.fail_count = 0;
        self.last_fail_at = None;
        self.last_fail_reason = None;
        if is_usage {
            self.use_count += 1;
            self.last_used_at = Some(chrono::Utc::now().timestamp_millis());
        }
        if self.quota == 0 {
            self.status = TokenStatus::Cooling;
        } else {
            self.status = TokenStatus::Active;
        }
    }

    pub fn need_refresh(&self, interval_hours: i64) -> bool {
        if self.status != TokenStatus::Cooling {
            return false;
        }
        if self.last_sync_at.is_none() {
            return true;
        }
        let now = chrono::Utc::now().timestamp_millis();
        let interval_ms = interval_hours * 3600 * 1000;
        now - self.last_sync_at.unwrap_or(0) >= interval_ms
    }

    pub fn mark_synced(&mut self) {
        self.last_sync_at = Some(chrono::Utc::now().timestamp_millis());
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenPoolStats {
    pub total: usize,
    pub active: usize,
    pub disabled: usize,
    pub expired: usize,
    pub cooling: usize,
    pub total_quota: i32,
    pub avg_quota: f64,
}

impl Default for TokenPoolStats {
    fn default() -> Self {
        Self {
            total: 0,
            active: 0,
            disabled: 0,
            expired: 0,
            cooling: 0,
            total_quota: 0,
            avg_quota: 0.0,
        }
    }
}
