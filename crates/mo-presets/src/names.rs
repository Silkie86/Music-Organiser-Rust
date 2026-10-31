//! Names made safe to save a file under: the C# app's `TextScan.SafeFileName`, for every export. (Kept here while
//! only the Presets page exports; it can move to `mo_formats::textscan` beside the rest of `TextScan` once another
//! export needs it.)

/// The longest name [`safe_file_name`] gives (UTF-16 units, as .NET counts): longer ones are cut to this.
pub const LONGEST_FILE_NAME: usize = 120;

/// A letter no system takes in a file's name: `< > : " / \ | ? *` and control characters.
pub fn unsafe_in_file_name(c: char) -> bool {
    matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c < ' '
}

/// Names Windows keeps for its devices, whatever ending follows ("CON", "aux.txt", "COM1"), letter case aside.
fn is_device_name(name: &str) -> bool {
    let base = name.split('.').next().unwrap_or_default().trim_end_matches(' ').to_ascii_uppercase();
    let numbered = |prefix: &str| {
        base.strip_prefix(prefix)
            .is_some_and(|n| matches!(n, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"))
    };
    matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$") || numbered("COM") || numbered("LPT")
}

/// A name every system can save a file under, as the C# `TextScan.SafeFileName`: the letters [`unsafe_in_file_name`]
/// become "_", spaces and dots are trimmed from the ends, it's cut to [`LONGEST_FILE_NAME`], and an empty name is
/// "untitled". Two things the C# app didn't do: a name cut where a space or dot was loses it too (Windows can't open a
/// folder whose name ends in one), and a Windows device's name ("Aux", "CON") gets a "_" in front.
pub fn safe_file_name(name: &str) -> String {
    let cleaned: String = name.chars().map(|c| if unsafe_in_file_name(c) { '_' } else { c }).collect();
    let trimmed = cleaned.trim_matches([' ', '.']);
    let named = if is_device_name(trimmed) { format!("_{trimmed}") } else { trimmed.to_owned() };
    let mut out = String::new();
    let mut units = 0;
    for c in named.chars() {
        units += c.len_utf16();
        if units > LONGEST_FILE_NAME {
            break;
        }
        out.push(c);
    }
    let kept = out.trim_end_matches([' ', '.']).len();
    out.truncate(kept);
    if out.is_empty() { "untitled".to_owned() } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_made_safe_as_the_c_sharp_app_makes_them() {
        assert_eq!(safe_file_name(r#"a<b>c:d"e/f\g|h?i*j"#), "a_b_c_d_e_f_g_h_i_j");
        assert_eq!(safe_file_name(" .Bass.. "), "Bass");
        assert_eq!(safe_file_name("tab\there"), "tab_here");
        assert_eq!(safe_file_name("..."), "untitled");
        assert_eq!(safe_file_name(&"x".repeat(200)).len(), 120);
        // Counted as .NET counts: an emoji is two.
        assert_eq!(safe_file_name(&"🎹".repeat(70)).chars().count(), 60);
    }

    #[test]
    fn a_cut_name_never_ends_in_a_space_or_dot_and_a_device_name_is_kept_clear() {
        assert_eq!(safe_file_name(&format!("{} {}", "x".repeat(119), "y".repeat(9))), "x".repeat(119));
        assert_eq!(safe_file_name(&format!("{}.{}", "x".repeat(119), "y".repeat(9))), "x".repeat(119));
        assert_eq!(safe_file_name("Aux"), "_Aux");
        assert_eq!(safe_file_name("con.backup"), "_con.backup");
        assert_eq!(safe_file_name("COM3"), "_COM3");
        assert_eq!(safe_file_name("Auxiliary"), "Auxiliary");
        assert_eq!(safe_file_name("COM10"), "COM10");
    }
}
