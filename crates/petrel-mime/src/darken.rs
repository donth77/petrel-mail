//! Light-only mail, recolored for a dark reading pane.
//!
//! Most HTML mail hard-codes a white world. A sender who declared dark
//! support keeps their own colors (`declares_dark`, elsewhere); everything
//! else rendered light-on-light as a white island in a dark app. This pass
//! rewrites the colors the sanitizer let through — style attributes and
//! `bgcolor` — flipping lightness and keeping hue: white grounds go dark,
//! near-black text goes light, a brand's teal stays that brand's teal.
//! Images are bytes, not declarations, and are never touched. It runs on
//! the sanitizer's own serialized output, whose attribute shapes are ours.

/// Rewrites every recolorable declaration for a dark ground.
pub fn recolor_for_dark(html: &str) -> String {
    let mut out = String::with_capacity(html.len() + 64);
    let mut rest = html;
    loop {
        // Attribute starts are the serializer's: lowercase name, `="`.
        let Some((idx, attr)) = ["style=\"", "bgcolor=\""]
            .iter()
            .filter_map(|a| rest.find(a).map(|i| (i, *a)))
            .min_by_key(|(i, _)| *i)
        else {
            out.push_str(rest);
            return out;
        };
        let start = idx + attr.len();
        let Some(len) = rest[start..].find('"') else {
            out.push_str(rest);
            return out;
        };
        out.push_str(&rest[..start]);
        let value = &rest[start..start + len];
        if attr == "bgcolor=\"" {
            out.push_str(&recolor_token(value).unwrap_or_else(|| value.to_string()));
        } else {
            out.push_str(&recolor_style(value));
        }
        rest = &rest[start + len..];
    }
}

/// Properties whose values may carry colors, after the sanitizer's filter.
const COLOR_PROPS: &[&str] = &[
    "color",
    "background-color",
    "border",
    "border-top",
    "border-bottom",
    "border-left",
    "border-right",
    "border-color",
];

fn recolor_style(style: &str) -> String {
    style
        .split(';')
        .map(|decl| {
            let Some((prop, value)) = decl.split_once(':') else {
                return decl.to_string();
            };
            if !COLOR_PROPS.contains(&prop.trim().to_ascii_lowercase().as_str()) {
                return decl.to_string();
            }
            // Border shorthands mix widths and styles with a color; each
            // whitespace-separated token is tried on its own and non-colors
            // pass through unchanged.
            let rewritten = value
                .split_whitespace()
                .map(|tok| recolor_token(tok).unwrap_or_else(|| tok.to_string()))
                .collect::<Vec<_>>()
                .join(" ");
            format!("{prop}: {rewritten}", prop = prop.trim())
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// One CSS color token flipped, or None when it is not a color we read.
fn recolor_token(tok: &str) -> Option<String> {
    let t = tok.trim();
    if let Some(hex) = t.strip_prefix('#') {
        let (r, g, b, a) = match hex.len() {
            3 => {
                let v: Vec<u8> = hex
                    .chars()
                    .map(|c| c.to_digit(16).map(|d| (d * 17) as u8))
                    .collect::<Option<_>>()?;
                (v[0], v[1], v[2], None)
            }
            6 | 8 => {
                let b = (0..hex.len() / 2)
                    .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok())
                    .collect::<Option<Vec<u8>>>()?;
                (b[0], b[1], b[2], b.get(3).copied())
            }
            _ => return None,
        };
        let (r, g, b) = flip(r, g, b);
        return Some(match a {
            Some(a) => format!("#{r:02x}{g:02x}{b:02x}{a:02x}"),
            None => format!("#{r:02x}{g:02x}{b:02x}"),
        });
    }
    let lower = t.to_ascii_lowercase();
    if let Some(args) = lower
        .strip_prefix("rgba(")
        .or_else(|| lower.strip_prefix("rgb("))
        .and_then(|s| s.strip_suffix(')'))
    {
        let parts: Vec<&str> = args.split(',').map(str::trim).collect();
        if parts.len() < 3 {
            return None;
        }
        let ch = |s: &str| -> Option<u8> {
            if let Some(p) = s.strip_suffix('%') {
                Some(((p.trim().parse::<f32>().ok()? / 100.0) * 255.0).round() as u8)
            } else {
                s.parse::<f32>()
                    .ok()
                    .map(|v| v.round().clamp(0.0, 255.0) as u8)
            }
        };
        let (r, g, b) = flip(ch(parts[0])?, ch(parts[1])?, ch(parts[2])?);
        return Some(match parts.get(3) {
            Some(a) => format!("rgba({r}, {g}, {b}, {a})"),
            None => format!("rgb({r}, {g}, {b})"),
        });
    }
    // The named colors mail actually uses; anything unrecognized is left
    // alone rather than guessed at.
    let named: &[(&str, (u8, u8, u8))] = &[
        ("white", (255, 255, 255)),
        ("black", (0, 0, 0)),
        ("gray", (128, 128, 128)),
        ("grey", (128, 128, 128)),
        ("silver", (192, 192, 192)),
        ("whitesmoke", (245, 245, 245)),
        ("ghostwhite", (248, 248, 255)),
        ("lightgray", (211, 211, 211)),
        ("lightgrey", (211, 211, 211)),
        ("darkgray", (169, 169, 169)),
        ("darkgrey", (169, 169, 169)),
        ("dimgray", (105, 105, 105)),
        ("dimgrey", (105, 105, 105)),
        ("gainsboro", (220, 220, 220)),
        ("red", (255, 0, 0)),
        ("green", (0, 128, 0)),
        ("blue", (0, 0, 255)),
        ("navy", (0, 0, 128)),
        ("yellow", (255, 255, 0)),
        ("orange", (255, 165, 0)),
        ("purple", (128, 0, 128)),
        ("teal", (0, 128, 128)),
        ("maroon", (128, 0, 0)),
    ];
    let (r, g, b) = named.iter().find(|(n, _)| *n == lower)?.1;
    let (r, g, b) = flip(r, g, b);
    Some(format!("#{r:02x}{g:02x}{b:02x}"))
}

/// Lightness flipped, hue and saturation held (RGB → HSL → RGB).
fn flip(r: u8, g: u8, b: u8) -> (u8, u8, u8) {
    let (rf, gf, bf) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    let max = rf.max(gf).max(bf);
    let min = rf.min(gf).min(bf);
    let l = (max + min) / 2.0;
    let d = max - min;
    let (h, s) = if d == 0.0 {
        (0.0, 0.0)
    } else {
        let s = d / (1.0 - (2.0 * l - 1.0).abs()).max(f32::EPSILON);
        let h = if max == rf {
            ((gf - bf) / d).rem_euclid(6.0)
        } else if max == gf {
            (bf - rf) / d + 2.0
        } else {
            (rf - gf) / d + 4.0
        } * 60.0;
        (h, s.min(1.0))
    };
    // The flip itself, eased off pure extremes: absolute black grounds and
    // absolute white text read harsher than a dark app's own surfaces.
    let l = (1.0 - l).clamp(0.04, 0.96);
    // HSL → RGB.
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = l - c / 2.0;
    let (rf, gf, bf) = match h {
        h if h < 60.0 => (c, x, 0.0),
        h if h < 120.0 => (x, c, 0.0),
        h if h < 180.0 => (0.0, c, x),
        h if h < 240.0 => (0.0, x, c),
        h if h < 300.0 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    (
        ((rf + m) * 255.0).round() as u8,
        ((gf + m) * 255.0).round() as u8,
        ((bf + m) * 255.0).round() as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_flips_dark_and_hue_survives() {
        // White ground goes near-black; black text goes near-white.
        let html = r#"<td style="background-color: #ffffff; color: #111111">x</td>"#;
        let out = recolor_for_dark(html);
        assert!(out.contains("background-color: #0a0a0a"), "{out}");
        assert!(out.contains("color: #eeeeee"), "{out}");
        // A saturated brand blue keeps its hue: red stays low, blue high.
        let out = recolor_for_dark(r#"<a style="color: #1155cc">y</a>"#);
        let hex = out.split("color: #").nth(1).unwrap()[..6].to_string();
        let r = u8::from_str_radix(&hex[0..2], 16).unwrap();
        let b = u8::from_str_radix(&hex[4..6], 16).unwrap();
        assert!(b > r, "hue held: {out}");
    }

    #[test]
    fn bgcolor_named_and_rgba_forms() {
        let out =
            recolor_for_dark(r#"<table bgcolor="white"><tr style="color: rgba(20, 20, 20, 0.9)">"#);
        assert!(out.contains(r##"bgcolor="#0a0a0a""##), "{out}");
        assert!(out.contains("rgba(23") || out.contains("rgba(2"), "{out}");
        assert!(out.contains(", 0.9)"), "alpha kept: {out}");
    }

    #[test]
    fn non_colors_pass_untouched() {
        let html = r#"<img src="cid:x"><td style="width: 600px; border: 1px solid #dddddd">z</td>"#;
        let out = recolor_for_dark(html);
        assert!(out.contains(r#"src="cid:x""#));
        assert!(out.contains("width: 600px"));
        assert!(out.contains("1px solid #"), "{out}");
        assert!(!out.contains("#dddddd"), "border color flipped: {out}");
        // Unknown named colors and keywords stay.
        let out = recolor_for_dark(r#"<td style="background-color: transparent">x</td>"#);
        assert!(out.contains("transparent"));
    }
}
