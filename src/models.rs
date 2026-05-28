use serde::{Deserialize, Deserializer};

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct WorkResponse {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub data: Vec<WorkItem>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct WorkItem {
    #[serde(default, deserialize_with = "string_from_any")]
    pub id: String,
    #[serde(default, deserialize_with = "string_from_any")]
    pub room_name: String,
    #[serde(default, deserialize_with = "string_from_any")]
    pub message: String,
    #[serde(default, deserialize_with = "string_from_any")]
    pub file_uuid: String,
    #[serde(default, deserialize_with = "string_from_any")]
    pub file_subname: String,
}

impl WorkItem {
    pub fn has_message(&self) -> bool {
        !self.message.trim().is_empty()
    }

    pub fn has_file(&self) -> bool {
        !self.file_uuid.trim().is_empty()
    }
}

pub fn is_image_data(data: &[u8]) -> bool {
    data.starts_with(&[0xff, 0xd8, 0xff])
        || data.starts_with(b"\x89PNG\r\n\x1a\n")
        || data.starts_with(b"GIF87a")
        || data.starts_with(b"GIF89a")
}

fn string_from_any<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    let text = match value {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(value) => value,
        serde_json::Value::Number(value) => value.to_string(),
        serde_json::Value::Bool(value) => value.to_string(),
        other => other.to_string(),
    };

    Ok(text.trim().to_string())
}
