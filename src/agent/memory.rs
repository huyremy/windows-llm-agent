use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone)]
pub struct Message {
    pub role: String,
    pub content: String,
    pub timestamp: DateTime<Utc>,
}

pub struct Memory {
    history: Vec<Message>,
    max_messages: usize,
}

impl Memory {
    pub fn new(max_messages: usize) -> Self {
        Self {
            history: Vec::new(),
            max_messages,
        }
    }

    pub fn append(&mut self, role: &str, content: &str) {
        self.history.push(Message {
            role: role.to_string(),
            content: content.to_string(),
            timestamp: Utc::now(),
        });
        
        // Giữ chỉ N tin nhắn gần nhất
        if self.history.len() > self.max_messages {
            self.history.drain(0..self.history.len() - self.max_messages);
        }
    }

    pub fn history(&self) -> String {
        self.history.iter().map(|m| {
            format!("[{}] {}: {}", m.timestamp.format("%H:%M"), m.role, m.content)
        }).collect::<Vec<_>>().join("\n")
    }
}
