// Ported from CodexBar Tests/CodexBarTests/ProviderDetailSectionTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_protocol::{
    Chart, ChartKind, ChartPoint, DetailRow, DetailSection, ProviderSnapshot, RowProgress,
};

#[test]
fn construction_trims_strings_and_preserves_valid_detail_data() {
    let point = ChartPoint::new(" Monday ", 12.5).expect("valid point");
    let chart = Chart::new(
        ChartKind::Bars,
        Some(" Daily usage "),
        Some(" USD "),
        vec![point.clone()],
    )
    .expect("valid chart");

    let row = DetailRow::new(
        None::<&str>,
        " Total ",
        " $12.50 ",
        Some(" 120 requests "),
        None,
        None,
    )
    .expect("valid row");

    let section = DetailSection::new(Some(" Billing "), vec![row.clone()], Some(chart.clone()))
        .expect("valid section");

    assert_eq!(section.title.as_deref(), Some("Billing"));
    assert_eq!(section.rows.len(), 1);
    assert_eq!(section.rows[0].label, "Total");
    assert_eq!(section.rows[0].value, "$12.50");
    assert_eq!(
        section.rows[0].secondary_value.as_deref(),
        Some("120 requests")
    );

    let section_chart = section.chart.as_ref().expect("chart present");
    assert_eq!(section_chart.title.as_deref(), Some("Daily usage"));
    assert_eq!(section_chart.unit.as_deref(), Some("USD"));
    assert_eq!(
        section_chart.points,
        vec![ChartPoint::new("Monday", 12.5).unwrap()]
    );
}

#[test]
fn construction_rejects_invalid_bounds_and_nonfinite_points() {
    let valid_row =
        DetailRow::new(None::<&str>, "Label", "Value", None::<&str>, None, None).unwrap();
    let valid_point = ChartPoint::new("Point", 1.0).unwrap();

    // Row label > 120 characters
    assert!(
        DetailRow::new(
            None::<&str>,
            "x".repeat(121),
            "Value",
            None::<&str>,
            None,
            None
        )
        .is_err()
    );

    // Chart point value infinite
    assert!(ChartPoint::new("Point", f64::INFINITY).is_err());

    // Section with > 24 rows
    let rows_25 = vec![valid_row; 25];
    assert!(DetailSection::new(None::<&str>, rows_25, None).is_err());

    // Chart with > 120 points
    let points_121 = vec![valid_point; 121];
    assert!(Chart::new(ChartKind::Line, None::<&str>, None::<&str>, points_121).is_err());
}

#[test]
fn decoding_rejects_too_many_sections() {
    let section_json = r#"{"rows":[]}"#;
    let details_json = (0..9).map(|_| section_json).collect::<Vec<_>>().join(",");
    let json = format!(
        r#"{{
            "id": "claude",
            "displayName": "Claude",
            "enabled": true,
            "sourceMode": "auto",
            "details": [{details_json}]
        }}"#
    );

    let res: Result<ProviderSnapshot, _> = serde_json::from_str(&json);
    assert!(res.is_err(), "expected rejection of 9 detail sections");
}

#[test]
fn row_progress_divides_used_by_total_without_clamping() {
    let progress = RowProgress::new(31.0, 3000.0).expect("valid progress");
    assert!((progress.used_percent() - 1.0333333).abs() < 0.0001);

    let over_100 = RowProgress::new(150.0, 100.0).expect("valid progress");
    assert_eq!(over_100.used_percent(), 150.0);
}

#[test]
fn row_progress_rejects_non_positive_totals_and_nonfinite_values() {
    assert!(RowProgress::new(31.0, 0.0).is_err());
    assert!(RowProgress::new(31.0, -5.0).is_err());
    assert!(RowProgress::new(f64::INFINITY, 100.0).is_err());
}

#[test]
fn row_id_progress_and_usage_value_round_trip_through_codable() {
    let progress = RowProgress::new(81.0, 6000.0).unwrap();
    let row = DetailRow::new(
        Some("copilot-seat-credits"),
        "Credits used",
        "81 / 6000",
        None::<&str>,
        Some(progress),
        Some(81.0),
    )
    .unwrap();

    let json = serde_json::to_string(&row).expect("serializes");
    let decoded: DetailRow = serde_json::from_str(&json).expect("deserializes");
    assert_eq!(decoded, row);
}

#[test]
fn row_usage_value_rejects_nonfinite_values() {
    assert!(
        DetailRow::new(
            None::<&str>,
            "Credits used",
            "31",
            None::<&str>,
            None,
            Some(f64::INFINITY)
        )
        .is_err()
    );

    assert!(
        DetailRow::new(
            None::<&str>,
            "Credits used",
            "31",
            None::<&str>,
            None,
            Some(f64::NAN)
        )
        .is_err()
    );
}

#[test]
fn row_without_id_and_progress_decodes_from_legacy_payloads() {
    let json = r#"{"label":"Credits used","value":"31","secondaryValue":"Resets Sep 1"}"#;
    let row: DetailRow = serde_json::from_str(json).expect("decodes legacy payload");

    assert_eq!(row.id, None);
    assert_eq!(row.progress, None);
    assert_eq!(row.usage_value, None);
    assert_eq!(row.label, "Credits used");
    assert_eq!(row.value, "31");
    assert_eq!(row.secondary_value.as_deref(), Some("Resets Sep 1"));
}

#[test]
fn current_snapshot_fixture_decodes_with_empty_details() {
    let json = r#"{
        "id": "synthetic",
        "displayName": "Synthetic",
        "enabled": true,
        "sourceMode": "auto",
        "details": []
    }"#;

    let snapshot: ProviderSnapshot = serde_json::from_str(json).expect("decodes snapshot");
    assert_eq!(snapshot.id.as_str(), "synthetic");
    assert!(snapshot.details.is_empty());

    let encoded = serde_json::to_string(&snapshot).expect("serializes snapshot");
    // Empty details should be omitted from JSON
    assert!(!encoded.contains("\"details\""));
}
