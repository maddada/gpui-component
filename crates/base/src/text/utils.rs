use std::sync::Arc;

use data_url::DataUrl;
use gpui::{Image, ImageFormat};

const LOWER_ALPHA: &str = "abcdefghijklmnopqrstuvwxyz";

const BULLETS: [&str; 5] = ["•", "◦", "▪", "‣", "⁃"];

const ROMAN: [(u32, &str); 13] = [
    (1000, "m"),
    (900, "cm"),
    (500, "d"),
    (400, "cd"),
    (100, "c"),
    (90, "xc"),
    (50, "l"),
    (40, "xl"),
    (10, "x"),
    (9, "ix"),
    (5, "v"),
    (4, "iv"),
    (1, "i"),
];

/// CSS's `lower-roman`, for the third level of numbering.
fn lower_roman(mut number: u32) -> String {
    let mut result = String::new();
    for (value, numeral) in ROMAN {
        while number >= value {
            result.push_str(numeral);
            number -= value;
        }
    }
    result
}

/// Returns an ordered-list item's ordinal, defaulting omitted starts to one.
pub(super) fn ordered_list_ordinal(start: Option<u32>, ix: usize) -> u32 {
    start
        .unwrap_or(1)
        .saturating_add(u32::try_from(ix).unwrap_or(u32::MAX))
}

/// Returns the prefix for a list item.
///
/// `depth` counts the lists of this item's own kind it sits inside, so the
/// sequences run the way the CSS defaults do: `decimal`, `lower-alpha`,
/// `lower-roman` for numbers, and `disc`, `circle`, `square` for bullets. A
/// bulleted list directly inside a numbered one is a first-level bulleted
/// list, as CSS's `ul ul` selector decides it.
pub(super) fn list_item_prefix(
    ix: usize,
    start: Option<u32>,
    ordered: bool,
    depth: usize,
) -> String {
    if ordered {
        let ordinal = ordered_list_ordinal(start, ix);
        // A list counting from zero has no letter or numeral for its first
        // item, so it keeps its numbers at every depth.
        if depth == 0 || start == Some(0) {
            return format!("{ordinal}. ");
        }
        if depth == 1 {
            let alpha_ix = ordinal.saturating_sub(1) as usize;
            return format!(
                "{}. ",
                LOWER_ALPHA
                    .chars()
                    .nth(alpha_ix % LOWER_ALPHA.len())
                    .unwrap()
            );
        }
        return format!("{}. ", lower_roman(ordinal));
    }
    format!("{} ", BULLETS[depth.min(BULLETS.len() - 1)])
}

/// Decodes a `data:` URL whose mime type names an image format GPUI can
/// decode. Anything else (another scheme, a non-image body, malformed
/// base64) yields `None`, and the caller keeps the URL URI-backed so GPUI's
/// own loader reports the failure.
pub(super) fn data_url_image(url: &str) -> Option<Arc<Image>> {
    let data_url = DataUrl::process(url).ok()?;
    let mime = data_url.mime_type();
    let format = ImageFormat::from_mime_type(&format!("{}/{}", mime.type_, mime.subtype))?;
    let (bytes, _fragment) = data_url.decode_to_vec().ok()?;
    Some(Arc::new(Image::from_bytes(format, bytes)))
}

#[cfg(test)]
mod tests {
    use gpui::ImageFormat;

    use crate::text::utils::{data_url_image, list_item_prefix};

    #[test]
    fn test_data_url_image() {
        fn image(url: &str) -> (ImageFormat, Vec<u8>) {
            let image = data_url_image(url)
                .unwrap_or_else(|| panic!("expected an embedded image for {url:?}"));
            (image.format(), image.bytes().to_vec())
        }

        assert_eq!(
            image("data:image/png;base64,iVBORw0KGgo="),
            (ImageFormat::Png, b"\x89PNG\r\n\x1a\n".to_vec())
        );
        // Legacy mime aliases and the percent-encoded (non-base64) body form.
        assert_eq!(
            image("data:image/jpg;base64,/9j/4A=="),
            (ImageFormat::Jpeg, b"\xff\xd8\xff\xe0".to_vec())
        );
        assert_eq!(
            image("data:image/svg+xml,%3Csvg%3E%3C/svg%3E"),
            (ImageFormat::Svg, b"<svg></svg>".to_vec())
        );

        assert!(data_url_image("https://example.com/logo.png").is_none());
        assert!(data_url_image("data:text/plain;base64,aGVsbG8=").is_none());
        assert!(data_url_image("data:image/png;base64,not*base64").is_none());
        assert!(data_url_image("data:image/png").is_none());
    }

    #[test]
    fn test_list_item_prefix() {
        assert_eq!(list_item_prefix(0, Some(1), true, 0), "1. ");
        assert_eq!(list_item_prefix(1, Some(1), true, 0), "2. ");
        assert_eq!(list_item_prefix(2, Some(1), true, 0), "3. ");
        assert_eq!(list_item_prefix(10, Some(1), true, 0), "11. ");
        assert_eq!(list_item_prefix(0, Some(3), true, 0), "3. ");
        assert_eq!(list_item_prefix(1, Some(3), true, 0), "4. ");
        assert_eq!(list_item_prefix(0, Some(1), true, 1), "a. ");
        assert_eq!(list_item_prefix(1, Some(1), true, 1), "b. ");
        assert_eq!(list_item_prefix(6, Some(1), true, 1), "g. ");
        assert_eq!(list_item_prefix(0, Some(4), true, 1), "d. ");
        assert_eq!(list_item_prefix(0, Some(1), true, 2), "i. ");
        assert_eq!(list_item_prefix(3, Some(1), true, 2), "iv. ");
        assert_eq!(list_item_prefix(8, Some(1), true, 2), "ix. ");
        assert_eq!(list_item_prefix(0, Some(0), true, 1), "0. ");
        assert_eq!(list_item_prefix(1, Some(0), true, 1), "1. ");
        assert_eq!(list_item_prefix(0, None, false, 0), "• ");
        assert_eq!(list_item_prefix(0, None, false, 1), "◦ ");
        assert_eq!(list_item_prefix(0, None, false, 2), "▪ ");
        assert_eq!(list_item_prefix(0, None, false, 3), "‣ ");
        assert_eq!(list_item_prefix(0, None, false, 4), "⁃ ");
    }
}
