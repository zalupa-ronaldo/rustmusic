/// Split a track subtitle into the artist line and the album shown beside the title.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caption {
    pub artist: String,
    pub album: Option<String>,
}
pub fn split_subtitle(subtitle: &str) -> Caption {
    let parts = subtitle
        .split(" · ")
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    match parts.len() {
        0 => Caption {
            artist: String::new(),
            album: None,
        },
        1 => Caption {
            artist: String::new(),
            album: Some(parts[0].to_owned()),
        },
        length => Caption {
            artist: parts[..length - 1].join(" · "),
            album: Some(parts[length - 1].to_owned()),
        },
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn album_is_the_label_beside_the_title() {
        let caption = split_subtitle("Artist · Album name");
        assert_eq!(caption.artist, "Artist");
        assert_eq!(caption.album.as_deref(), Some("Album name"));
        let only = split_subtitle("  Single label  ");
        assert!(only.artist.is_empty());
        assert_eq!(only.album.as_deref(), Some("Single label"));
        assert_eq!(split_subtitle("   "), Caption { artist: String::new(), album: None });
        let nested = split_subtitle("A · B · C");
        assert_eq!(nested.artist, "A · B");
        assert_eq!(nested.album.as_deref(), Some("C"));
    }
}
