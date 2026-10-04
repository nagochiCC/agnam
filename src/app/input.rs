use crate::settings::ClickMode;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum ClickDirection {
    Next,
    Prev,
    None,
}

pub(super) fn decide_click_direction(
    click_mode: ClickMode,
    button: u32,
    x: f64,
    width: i32,
) -> ClickDirection {
    match click_mode {
        ClickMode::ButtonBased => match button {
            1 => ClickDirection::Next,
            3 => ClickDirection::Prev,
            _ => ClickDirection::None,
        },
        ClickMode::AreaBased => {
            if x < (f64::from(width) / 2.0) {
                ClickDirection::Next
            } else {
                ClickDirection::Prev
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_click_mode_uses_primary_and_secondary_buttons() {
        assert_eq!(
            decide_click_direction(ClickMode::ButtonBased, 1, 100.0, 200),
            ClickDirection::Next
        );
        assert_eq!(
            decide_click_direction(ClickMode::ButtonBased, 3, 100.0, 200),
            ClickDirection::Prev
        );
        assert_eq!(
            decide_click_direction(ClickMode::ButtonBased, 2, 0.0, 200),
            ClickDirection::None
        );
    }

    #[test]
    fn area_click_mode_splits_at_the_exact_center() {
        assert_eq!(
            decide_click_direction(ClickMode::AreaBased, 2, 99.9, 200),
            ClickDirection::Next
        );
        assert_eq!(
            decide_click_direction(ClickMode::AreaBased, 2, 100.0, 200),
            ClickDirection::Prev
        );
        assert_eq!(
            decide_click_direction(ClickMode::AreaBased, 1, 150.0, 200),
            ClickDirection::Prev
        );
    }
}
