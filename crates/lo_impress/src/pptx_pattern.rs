//! LibreOffice-compatible hatch approximations for DrawingML patterns.
//! Mapping derived from LibreOffice oox/inc/drawingml/hatchmap.hxx.
//! This file is licensed under MPL-2.0; see data/LIBREOFFICE-MPL-2.0.
//! Decorative bitmap patterns intentionally follow the reference's hatch mapping.
pub(crate) fn hatch(preset: &str, color: String) -> Option<lo_core::style::HatchFill> {
    let (count, distance, angle) = match preset {
        "pct5" => (1, 250, 45),
        "pct10" => (1, 200, 45),
        "pct20" | "dashUpDiag" => (1, 150, 45),
        "pct25" => (2, 200, 45),
        "pct30" => (2, 175, 45),
        "pct40" | "weave" => (2, 150, 45),
        "pct50" => (2, 125, 45),
        "pct60" => (3, 150, 45),
        "pct70" => (3, 125, 45),
        "pct75" => (3, 100, 45),
        "pct80" => (3, 75, 45),
        "pct90" => (3, 50, 45),
        "horz" | "wave" => (1, 100, 0),
        "vert" => (1, 100, 90),
        "ltHorz" | "narHorz" => (1, 50, 0),
        "ltVert" | "narVert" => (1, 50, 90),
        "dkHorz" => (1, 25, 0),
        "dkVert" => (1, 25, 90),
        "dashHorz" => (1, 150, 0),
        "dashVert" => (1, 150, 90),
        "cross" | "lgGrid" => (2, 100, 0),
        "dnDiag" | "wdDnDiag" => (1, 100, 135),
        "upDiag" | "wdUpDiag" => (1, 100, 45),
        "ltDnDiag" | "dkDnDiag" => (1, 50, 135),
        "ltUpDiag" | "dkUpDiag" => (1, 50, 45),
        "dashDnDiag" => (1, 150, 135),
        "diagCross" => (2, 100, 45),
        "smCheck" => (2, 50, 45),
        "lgCheck" => (2, 100, 45),
        "smGrid" => (2, 50, 0),
        "dotGrid" => (2, 400, 0),
        "smConfetti" => (1, 200, 60),
        "lgConfetti" => (1, 100, 60),
        "horzBrick" => (2, 300, 0),
        "diagBrick" | "dotDmnd" => (2, 300, 45),
        "solidDmnd" => (2, 100, 45),
        "openDmnd" => (2, 100, 45),
        "plaid" => (3, 200, 90),
        "sphere" => (3, 100, 0),
        "divot" => (3, 400, 45),
        "shingle" => (1, 200, 135),
        "trellis" => (2, 75, 45),
        "zigZag" => (1, 75, 0),
        _ => return None,
    };
    Some(lo_core::style::HatchFill {
        color,
        distance_mpt: ((distance as f32 / 100.0) * 72.0 / 25.4 * 1000.0).round() as u32,
        angle_degrees: angle,
        line_count: count,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diagonal_hatches_keep_reference_spacing_and_direction() {
        let down = hatch("ltDnDiag", "#000000".into()).unwrap();
        let up = hatch("ltUpDiag", "#000000".into()).unwrap();
        assert_eq!(down.angle_degrees, 135);
        assert_eq!(up.angle_degrees, 45);
        assert_eq!(down.distance_mpt, 1417);
        assert_eq!(hatch("pct50", "#000000".into()).unwrap().line_count, 2);
        assert!(hatch("invalid", String::new()).is_none());
    }
}
