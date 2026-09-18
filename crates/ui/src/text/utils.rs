const LOWER_ALPHA: &str = "abcdefghijklmnopqrstuvwxyz";

const BULLETS: [&str; 5] = ["•", "◦", "▪", "‣", "⁃"];

const ROMAN: [(usize, &str); 13] = [
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
fn lower_roman(mut number: usize) -> String {
    let mut result = String::new();
    for (value, numeral) in ROMAN {
        while number >= value {
            result.push_str(numeral);
            number -= value;
        }
    }
    result
}

/// Returns the prefix for a list item.
///
/// `depth` counts the lists of this item's own kind it sits inside, so the sequences run the way
/// the CSS defaults do: `decimal`, `lower-alpha`, `lower-roman` for numbers, and `disc`, `circle`,
/// `square` for bullets.
pub(super) fn list_item_prefix(ix: usize, ordered: bool, depth: usize) -> String {
    if ordered {
        return match depth {
            0 => format!("{}. ", ix + 1),
            1 => format!(
                "{}. ",
                LOWER_ALPHA.chars().nth(ix % LOWER_ALPHA.len()).unwrap()
            ),
            _ => format!("{}. ", lower_roman(ix + 1)),
        };
    }
    format!("{} ", BULLETS[depth.min(BULLETS.len() - 1)])
}

#[cfg(test)]
mod tests {
    use crate::text::utils::list_item_prefix;

    #[test]
    fn test_list_item_prefix() {
        assert_eq!(list_item_prefix(0, true, 0), "1. ");
        assert_eq!(list_item_prefix(1, true, 0), "2. ");
        assert_eq!(list_item_prefix(2, true, 0), "3. ");
        assert_eq!(list_item_prefix(10, true, 0), "11. ");
        assert_eq!(list_item_prefix(0, true, 1), "a. ");
        assert_eq!(list_item_prefix(1, true, 1), "b. ");
        assert_eq!(list_item_prefix(6, true, 1), "g. ");
        assert_eq!(list_item_prefix(0, true, 2), "i. ");
        assert_eq!(list_item_prefix(3, true, 2), "iv. ");
        assert_eq!(list_item_prefix(8, true, 2), "ix. ");
        assert_eq!(list_item_prefix(0, false, 0), "• ");
        assert_eq!(list_item_prefix(0, false, 1), "◦ ");
        assert_eq!(list_item_prefix(0, false, 2), "▪ ");
        assert_eq!(list_item_prefix(0, false, 3), "‣ ");
        assert_eq!(list_item_prefix(0, false, 4), "⁃ ");
    }
}
