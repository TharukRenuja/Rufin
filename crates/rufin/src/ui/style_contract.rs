const APP_STYLE: &str = include_str!("../style.css");
const SECONDARY_METADATA_COLOR: &str = "color-mix(in srgb, @window_fg_color 95%, @window_bg_color)";
const METADATA_LINK_HOVER_COLOR: &str = "color-mix(in srgb, @accent_color 72%, @window_fg_color)";
const TOOLTIP_BACKGROUND: &str = "rgba(0, 0, 0, 0.88)";
const TOOLTIP_FOREGROUND: &str = "white";
const TOAST_BACKGROUND: &str = "color-mix(in srgb, @window_bg_color 88%, @card_bg_color 12%)";
const TOAST_FOREGROUND: &str = "@window_fg_color";
const TOAST_RADIUS: &str = "999px";
const TOAST_BOTTOM_MARGIN: &str = "108px";
const TOAST_CLOSE_OPACITY: &str = "0";
const TOAST_CLOSE_ICON_SIZE: &str = "1px";
const MIN_BODY_TEXT_CONTRAST: f64 = 4.5;

#[derive(Clone, Copy)]
struct Rgb {
    red: f64,
    green: f64,
    blue: f64,
}

#[derive(Clone, Copy)]
struct ThemePalette {
    name: &'static str,
    window_bg: Rgb,
    window_fg: Rgb,
    accent: Rgb,
}

fn light_palette() -> ThemePalette {
    ThemePalette {
        name: "light",
        window_bg: Rgb::from_rgb8(255, 255, 255),
        window_fg: Rgb::from_rgb8(36, 31, 49),
        accent: Rgb::from_rgb8(53, 132, 228),
    }
}

fn dark_palette() -> ThemePalette {
    ThemePalette {
        name: "dark",
        window_bg: Rgb::from_rgb8(36, 36, 36),
        window_fg: Rgb::from_rgb8(255, 255, 255),
        accent: Rgb::from_rgb8(153, 193, 241),
    }
}

fn secondary_metadata_color(palette: ThemePalette) -> Rgb {
    palette.window_fg.mix(palette.window_bg, 0.95)
}

fn metadata_link_hover_color(palette: ThemePalette) -> Rgb {
    palette.accent.mix(palette.window_fg, 0.72)
}

impl Rgb {
    fn from_rgb8(red: u8, green: u8, blue: u8) -> Self {
        Self {
            red: f64::from(red) / 255.0,
            green: f64::from(green) / 255.0,
            blue: f64::from(blue) / 255.0,
        }
    }

    fn mix(self, other: Self, weight: f64) -> Self {
        let other_weight = 1.0 - weight;
        Self {
            red: self.red * weight + other.red * other_weight,
            green: self.green * weight + other.green * other_weight,
            blue: self.blue * weight + other.blue * other_weight,
        }
    }

    fn contrast_ratio(self, other: Self) -> f64 {
        let foreground = self.relative_luminance();
        let background = other.relative_luminance();
        let lighter = foreground.max(background);
        let darker = foreground.min(background);
        (lighter + 0.05) / (darker + 0.05)
    }

    fn relative_luminance(self) -> f64 {
        0.2126 * linear_component(self.red)
            + 0.7152 * linear_component(self.green)
            + 0.0722 * linear_component(self.blue)
    }
}

fn linear_component(component: f64) -> f64 {
    if component <= 0.03928 {
        component / 12.92
    } else {
        ((component + 0.055) / 1.055).powf(2.4)
    }
}

fn selector_color(css: &str, selector: &str) -> Option<String> {
    selector_property(css, selector, "color")
}

fn selector_property(css: &str, selector: &str, property: &str) -> Option<String> {
    let mut remaining = css;
    while let Some(block_start) = remaining.find('{') {
        let selector_list = remaining.get(..block_start).unwrap_or_default().trim();
        let after_block_start = remaining.get(block_start + 1..).unwrap_or_default();
        let block_end = after_block_start.find('}')?;
        let block = after_block_start.get(..block_end).unwrap_or_default();
        if selector_list
            .split(',')
            .map(str::trim)
            .any(|candidate| candidate == selector)
        {
            return block
                .lines()
                .map(str::trim)
                .find_map(|line| line.strip_prefix(&format!("{property}:")))
                .map(|value| value.trim().trim_end_matches(';').to_string());
        }
        remaining = after_block_start.get(block_end + 1..).unwrap_or_default();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style_secondary_themes() {
        for selector in [
            ".muted",
            ".home-showcase .muted",
            ".album-detail-showcase .muted",
            ".server-section-label",
        ] {
            assert_eq!(
                selector_color(APP_STYLE, selector),
                Some(SECONDARY_METADATA_COLOR.to_string()),
                "{selector} should use the secondary metadata color"
            );
        }

        for palette in [light_palette(), dark_palette()] {
            let contrast = secondary_metadata_color(palette).contrast_ratio(palette.window_bg);
            assert!(
                contrast >= MIN_BODY_TEXT_CONTRAST,
                "{} secondary metadata contrast {contrast:.2} is below {MIN_BODY_TEXT_CONTRAST}",
                palette.name
            );
        }
    }

    #[test]
    fn style_meta_themes() {
        for selector in [".table-link:hover .table-link-label", ".hovered-link"] {
            assert_eq!(
                selector_color(APP_STYLE, selector),
                Some(METADATA_LINK_HOVER_COLOR.to_string()),
                "{selector} should use the metadata hover color"
            );
        }

        for palette in [light_palette(), dark_palette()] {
            let contrast = metadata_link_hover_color(palette).contrast_ratio(palette.window_bg);
            assert!(
                contrast >= MIN_BODY_TEXT_CONTRAST,
                "{} metadata hover contrast {contrast:.2} is below {MIN_BODY_TEXT_CONTRAST}",
                palette.name
            );
        }
    }

    #[test]
    fn style_tooltip_background() {
        assert_eq!(
            selector_property(APP_STYLE, "tooltip.background", "background"),
            Some(TOOLTIP_BACKGROUND.to_string())
        );
        assert_eq!(
            selector_color(APP_STYLE, "tooltip.background"),
            Some(TOOLTIP_FOREGROUND.to_string())
        );
        assert_eq!(
            selector_color(APP_STYLE, "tooltip.background label"),
            Some(TOOLTIP_FOREGROUND.to_string())
        );

        let contrast = Rgb::from_rgb8(255, 255, 255).contrast_ratio(Rgb::from_rgb8(31, 31, 31));
        assert!(
            contrast >= MIN_BODY_TEXT_CONTRAST,
            "tooltip contrast {contrast:.2} is below {MIN_BODY_TEXT_CONTRAST}"
        );
    }

    #[test]
    fn style_toasts() {
        assert_eq!(
            selector_property(APP_STYLE, "toastoverlay toast", "background"),
            Some(TOAST_BACKGROUND.to_string())
        );
        assert_eq!(
            selector_color(APP_STYLE, "toastoverlay toast"),
            Some(TOAST_FOREGROUND.to_string())
        );
        assert_eq!(
            selector_property(APP_STYLE, "toastoverlay toast", "border-radius"),
            Some(TOAST_RADIUS.to_string())
        );
        assert_eq!(
            selector_color(APP_STYLE, "toastoverlay toast label"),
            Some(TOAST_FOREGROUND.to_string())
        );
        assert_eq!(
            selector_property(APP_STYLE, ".app-toast-overlay toast", "margin-bottom"),
            Some(TOAST_BOTTOM_MARGIN.to_string())
        );
        assert_eq!(
            selector_property(
                APP_STYLE,
                ".quick-toast-overlay toast button.circular.flat",
                "opacity"
            ),
            Some(TOAST_CLOSE_OPACITY.to_string())
        );
        assert_eq!(
            selector_property(
                APP_STYLE,
                ".quick-toast-overlay toast button.circular.flat image",
                "-gtk-icon-size"
            ),
            Some(TOAST_CLOSE_ICON_SIZE.to_string())
        );
    }
}
