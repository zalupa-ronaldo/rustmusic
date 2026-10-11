use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Item {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub kind: ItemKind,
    pub thumbnail: Option<String>,
    pub duration: Option<String>,
    pub set_video_id: Option<String>,
}
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    #[default]
    Track,
    Album,
    Artist,
    Playlist,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Page {
    pub items: Vec<Item>,
    pub continuation: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub name: String,
    pub handle: String,
    pub auth_user: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Mutation {
    Like {
        video_id: String,
        liked: bool,
    },
    Create {
        title: String,
    },
    Rename {
        playlist_id: String,
        title: String,
    },
    Delete {
        playlist_id: String,
    },
    Add {
        playlist_id: String,
        video_id: String,
    },
    Remove {
        playlist_id: String,
        video_id: String,
        set_video_id: String,
    },
    Move {
        playlist_id: String,
        set_video_id: String,
        before_set_video_id: String,
    },
}

pub fn valid_video_id(id: &str) -> bool {
    id.len() == 11
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
pub fn valid_browse_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 256
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
