use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Basic,
    Super,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Cost {
    Low,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub model_id: String,
    pub grok_model: String,
    pub model_mode: String,
    pub tier: Tier,
    pub cost: Cost,
    pub display_name: String,
    pub description: String,
    pub is_video: bool,
    pub is_image: bool,
    pub is_image_edit: bool,
}

impl ModelInfo {
    pub fn new(model_id: &str, grok_model: &str, mode: &str, display: &str) -> Self {
        Self {
            model_id: model_id.to_string(),
            grok_model: grok_model.to_string(),
            model_mode: mode.to_string(),
            tier: Tier::Basic,
            cost: Cost::Low,
            display_name: display.to_string(),
            description: String::new(),
            is_video: false,
            is_image: false,
            is_image_edit: false,
        }
    }
}

pub struct ModelService;

impl ModelService {
    pub fn list() -> Vec<ModelInfo> {
        vec![
            // grok-3 系列
            ModelInfo::new("grok-3", "grok-3", "MODEL_MODE_GROK_3", "Grok 3"),
            ModelInfo::new("grok-3-fast", "grok-3", "MODEL_MODE_FAST", "Grok 3 Fast"),
            ModelInfo::new(
                "grok-3-mini",
                "grok-3",
                "MODEL_MODE_GROK_3_MINI_THINKING",
                "Grok 3 Mini",
            ),
            ModelInfo::new(
                "grok-3-thinking",
                "grok-3",
                "MODEL_MODE_GROK_3_THINKING",
                "Grok 3 Thinking",
            ),
            // grok-4 系列
            ModelInfo::new("grok-4", "grok-4", "MODEL_MODE_GROK_4", "Grok 4"),
            ModelInfo::new(
                "grok-4-mini",
                "grok-4-mini",
                "MODEL_MODE_GROK_4_MINI_THINKING",
                "Grok 4 Mini",
            ),
            ModelInfo::new("grok-4-fast", "grok-4", "MODEL_MODE_FAST", "Grok 4 Fast"),
            ModelInfo::new(
                "grok-4-thinking",
                "grok-4",
                "MODEL_MODE_GROK_4_THINKING",
                "Grok 4 Thinking",
            ),
            {
                let mut m =
                    ModelInfo::new("grok-4-heavy", "grok-4", "MODEL_MODE_HEAVY", "Grok 4 Heavy");
                m.tier = Tier::Super;
                m.cost = Cost::High;
                m
            },
            // grok-4.1 系列
            ModelInfo::new(
                "grok-4.1",
                "grok-4-1-thinking-1129",
                "MODEL_MODE_AUTO",
                "Grok 4.1",
            ),
            {
                let mut m = ModelInfo::new(
                    "grok-4.1-thinking",
                    "grok-4-1-thinking-1129",
                    "MODEL_MODE_GROK_4_1_THINKING",
                    "Grok 4.1 Thinking",
                );
                m.cost = Cost::High;
                m
            },
            ModelInfo::new(
                "grok-4.1-mini",
                "grok-4-1-thinking-1129",
                "MODEL_MODE_GROK_4_1_MINI_THINKING",
                "Grok 4.1 Mini",
            ),
            ModelInfo::new(
                "grok-4.1-fast",
                "grok-4-1-thinking-1129",
                "MODEL_MODE_FAST",
                "Grok 4.1 Fast",
            ),
            {
                let mut m = ModelInfo::new(
                    "grok-4.1-expert",
                    "grok-4-1-thinking-1129",
                    "MODEL_MODE_EXPERT",
                    "Grok 4.1 Expert",
                );
                m.cost = Cost::High;
                m
            },
            // grok-4.20
            ModelInfo::new(
                "grok-4.20-beta",
                "grok-420",
                "MODEL_MODE_GROK_420",
                "Grok 4.20 Beta",
            ),
            // 图片生成
            {
                let mut m = ModelInfo::new(
                    "grok-imagine-1.0",
                    "grok-3",
                    "MODEL_MODE_FAST",
                    "Grok Image",
                );
                m.cost = Cost::High;
                m.is_image = true;
                m.description = "Image generation model".to_string();
                m
            },
            // 图片编辑
            {
                let mut m = ModelInfo::new(
                    "grok-imagine-1.0-edit",
                    "imagine-image-edit",
                    "MODEL_MODE_FAST",
                    "Grok Image Edit",
                );
                m.cost = Cost::High;
                m.is_image_edit = true;
                m.description = "Image editing model".to_string();
                m
            },
            // 视频生成
            {
                let mut m = ModelInfo::new(
                    "grok-imagine-1.0-video",
                    "grok-3",
                    "MODEL_MODE_FAST",
                    "Grok Video",
                );
                m.cost = Cost::High;
                m.is_video = true;
                m.description = "Video generation model".to_string();
                m
            },
        ]
    }

    pub fn get(model_id: &str) -> Option<ModelInfo> {
        Self::list().into_iter().find(|m| m.model_id == model_id)
    }

    pub fn valid(model_id: &str) -> bool {
        Self::get(model_id).is_some()
    }

    /// 返回模型对应的候选 token 池列表（按优先级排序）
    pub fn pool_candidates_for_model(model_id: &str) -> Vec<&'static str> {
        if let Some(m) = Self::get(model_id) {
            if m.tier == Tier::Super {
                return vec!["ssoSuper"];
            }
        }
        // Basic tier：优先 ssoBasic，回退 ssoSuper
        vec!["ssoBasic", "ssoSuper"]
    }
}
