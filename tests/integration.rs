use std::path::{Path, PathBuf};

use board_interface::{
    Author, Change, ChangeKind, Classification, Driver, Element, ElementKind, Error, Handover,
    Hole, Keepout, LAYER_CUTOUT, LAYER_HOLE, LAYER_KEEPOUT, LAYER_OUTLINE, Loop, Manifest,
    NM_PER_MM, Point, Record, Role, SCHEMA_VERSION, Side, compare, hash, read, within, write,
};

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("golden")
}

fn check_or_update_golden_bytes(rel_path: &str, actual: &[u8]) {
    let path = golden_dir().join(rel_path);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().expect("parent dir")).expect("mkdir golden");
        std::fs::write(&path, actual).expect("write golden");
    }
    let expected = std::fs::read(&path).unwrap_or_else(|e| {
        panic!(
            "missing golden file {}: {e} (run with UPDATE_GOLDEN=1)",
            path.display()
        )
    });
    assert_eq!(expected, actual, "golden mismatch in {}", path.display());
}

fn make_manifest(board: &str, driver: Driver, thickness_mm: f64) -> Manifest {
    Manifest {
        schema: SCHEMA_VERSION,
        board: board.to_string(),
        revision: String::new(),
        parent: None,
        author: Author {
            app: driver,
            source: "0123456789abcdef0123456789abcdef01234567".to_string(),
            at: "2026-11-03T12:00:00Z".to_string(),
        },
        driver,
        last_handover: None,
        thickness_mm,
        geometry_hash: String::new(),
        approximate: false,
    }
}

fn finalize_record(mut record: Record) -> Record {
    let h = hash(&record);
    record.manifest.geometry_hash = h.clone();
    if record.manifest.revision.is_empty() {
        record.manifest.revision = h;
    }
    record
}

/// Fixture 1: 50 mm diameter round board centred at (25, 25) mm, 12 mm central circular cutout,
/// four odd-nanometre (2.700001 mm) mounting holes, and a front-side circular keepout.
fn build_round_board() -> Record {
    let center = Point::new(25 * NM_PER_MM, 25 * NM_PER_MM);
    let outline = Loop::new(vec![Element::new(
        Some("waffle:8b0e2a10-3c91-4d52-9a14-71f02a890001/1".to_string()),
        ElementKind::Circle {
            center,
            diameter_nm: 50 * NM_PER_MM,
        },
    )]);
    let cutouts = vec![Loop::new(vec![Element::new(
        Some("waffle:8b0e2a10-3c91-4d52-9a14-71f02a890001/2".to_string()),
        ElementKind::Circle {
            center,
            diameter_nm: 12 * NM_PER_MM,
        },
    )])];
    let holes = [
        ("3", (25_000_000, 5_000_000), true, 250_000),
        ("4", (45_000_000, 25_000_000), true, 250_000),
        ("5", (25_000_000, 45_000_000), false, 400_000),
        ("6", (5_000_000, 25_000_000), false, 400_000),
    ]
    .into_iter()
    .map(|(eid, (x, y), plated, clearance_nm)| Hole {
        id: Some(format!("waffle:8b0e2a10-3c91-4d52-9a14-71f02a890001/{eid}")),
        center: Point::new(x, y),
        // Odd nanometres: 2.700001 mm diameter -> 1.350000500 mm radius in DXF.
        diameter_nm: 2_700_001,
        plated,
        clearance_nm,
    })
    .collect();
    let keepouts = vec![Keepout {
        boundary: Loop::new(vec![Element::new(
            Some("waffle:8b0e2a10-3c91-4d52-9a14-71f02a890001/7".to_string()),
            ElementKind::Circle {
                center: Point::new(25 * NM_PER_MM, 14 * NM_PER_MM),
                diameter_nm: 6 * NM_PER_MM,
            },
        )]),
        side: Side::Front,
        max_height_nm: Some(3_500_000),
    }];

    finalize_record(Record {
        outline,
        cutouts,
        holes,
        keepouts,
        manifest: make_manifest("round_sensor", Driver::Waffle, 1.6),
    })
}

/// Fixture 2: 60 × 40 mm rectangle `[0, 60] × [0, 40]` mm with 4 mm radius arc corners, a stadium
/// slot cutout, two mounting holes, and a back-side rectangular keepout.
fn build_rounded_rect() -> Record {
    let mm = |v: i64| v * NM_PER_MM;
    // Walked clockwise on a Y-down screen (positive signed area / `angle_increasing = true` at each corner):
    // top edge (4,0)->(56,0), top-right arc ->(60,4), right edge ->(60,36), bottom-right arc ->(56,40),
    // bottom edge ->(4,40), bottom-left arc ->(0,36), left edge ->(0,4), top-left arc ->(4,0).
    let outline = Loop::new(vec![
        Element::new(
            Some("composer:out-top".to_string()),
            ElementKind::Line {
                start: Point::new(mm(4), mm(0)),
                end: Point::new(mm(56), mm(0)),
            },
        ),
        Element::new(
            Some("composer:out-tr".to_string()),
            ElementKind::Arc {
                center: Point::new(mm(56), mm(4)),
                start: Point::new(mm(56), mm(0)),
                end: Point::new(mm(60), mm(4)),
                angle_increasing: true,
            },
        ),
        Element::new(
            Some("composer:out-right".to_string()),
            ElementKind::Line {
                start: Point::new(mm(60), mm(4)),
                end: Point::new(mm(60), mm(36)),
            },
        ),
        Element::new(
            Some("composer:out-br".to_string()),
            ElementKind::Arc {
                center: Point::new(mm(56), mm(36)),
                start: Point::new(mm(60), mm(36)),
                end: Point::new(mm(56), mm(40)),
                angle_increasing: true,
            },
        ),
        Element::new(
            Some("composer:out-bottom".to_string()),
            ElementKind::Line {
                start: Point::new(mm(56), mm(40)),
                end: Point::new(mm(4), mm(40)),
            },
        ),
        Element::new(
            Some("composer:out-bl".to_string()),
            ElementKind::Arc {
                center: Point::new(mm(4), mm(36)),
                start: Point::new(mm(4), mm(40)),
                end: Point::new(mm(0), mm(36)),
                angle_increasing: true,
            },
        ),
        Element::new(
            Some("composer:out-left".to_string()),
            ElementKind::Line {
                start: Point::new(mm(0), mm(36)),
                end: Point::new(mm(0), mm(4)),
            },
        ),
        Element::new(
            Some("composer:out-tl".to_string()),
            ElementKind::Arc {
                center: Point::new(mm(4), mm(4)),
                start: Point::new(mm(0), mm(4)),
                end: Point::new(mm(4), mm(0)),
                angle_increasing: true,
            },
        ),
    ]);

    // Stadium cutout centred at (30, 20) mm: straight span x in [25, 35], y in [18, 22], r = 2 mm.
    let cutouts = vec![Loop::new(vec![
        Element::new(
            Some("composer:slot-top".to_string()),
            ElementKind::Line {
                start: Point::new(mm(25), mm(18)),
                end: Point::new(mm(35), mm(18)),
            },
        ),
        Element::new(
            Some("composer:slot-right".to_string()),
            ElementKind::Arc {
                center: Point::new(mm(35), mm(20)),
                start: Point::new(mm(35), mm(18)),
                end: Point::new(mm(35), mm(22)),
                angle_increasing: true,
            },
        ),
        Element::new(
            Some("composer:slot-bottom".to_string()),
            ElementKind::Line {
                start: Point::new(mm(35), mm(22)),
                end: Point::new(mm(25), mm(22)),
            },
        ),
        Element::new(
            Some("composer:slot-left".to_string()),
            ElementKind::Arc {
                center: Point::new(mm(25), mm(20)),
                start: Point::new(mm(25), mm(22)),
                end: Point::new(mm(25), mm(18)),
                angle_increasing: true,
            },
        ),
    ])];

    let holes = vec![
        Hole {
            id: Some("composer:mh1".to_string()),
            center: Point::new(mm(6), mm(6)),
            diameter_nm: 3_200_000,
            plated: true,
            clearance_nm: 300_000,
        },
        Hole {
            id: Some("composer:mh2".to_string()),
            center: Point::new(mm(54), mm(34)),
            diameter_nm: 3_200_000,
            plated: true,
            clearance_nm: 300_000,
        },
    ];

    let keepouts = vec![Keepout {
        boundary: Loop::new(vec![
            Element::new(
                Some("composer:ko-1".to_string()),
                ElementKind::Line {
                    start: Point::new(mm(10), mm(28)),
                    end: Point::new(mm(20), mm(28)),
                },
            ),
            Element::new(
                Some("composer:ko-2".to_string()),
                ElementKind::Line {
                    start: Point::new(mm(20), mm(28)),
                    end: Point::new(mm(20), mm(36)),
                },
            ),
            Element::new(
                Some("composer:ko-3".to_string()),
                ElementKind::Line {
                    start: Point::new(mm(20), mm(36)),
                    end: Point::new(mm(10), mm(36)),
                },
            ),
            Element::new(
                Some("composer:ko-4".to_string()),
                ElementKind::Line {
                    start: Point::new(mm(10), mm(36)),
                    end: Point::new(mm(10), mm(28)),
                },
            ),
        ]),
        side: Side::Back,
        max_height_nm: Some(2_000_000),
    }];

    finalize_record(Record {
        outline,
        cutouts,
        holes,
        keepouts,
        manifest: make_manifest("rounded_carrier", Driver::Composer, 1.6),
    })
}

#[test]
fn golden_valid_boards_round_trip_byte_for_byte_and_preserve_record_and_hash() {
    let round = build_round_board();
    let (round_dxf, round_json) = write(&round);
    check_or_update_golden_bytes("round_board.dxf", round_dxf.as_bytes());
    check_or_update_golden_bytes("round_board.json", round_json.as_bytes());
    let round_back = read(&round_dxf, &round_json).expect("read round_board");
    assert_eq!(round_back, round);
    assert_eq!(hash(&round_back), hash(&round));

    let rect = build_rounded_rect();
    let (rect_dxf, rect_json) = write(&rect);
    check_or_update_golden_bytes("rounded_rect.dxf", rect_dxf.as_bytes());
    check_or_update_golden_bytes("rounded_rect.json", rect_json.as_bytes());
    let rect_back = read(&rect_dxf, &rect_json).expect("read rounded_rect");
    assert_eq!(rect_back, rect);
    assert_eq!(hash(&rect_back), hash(&rect));

    // easy2350 is produced by `tools/convert_easy2350.py`; verify our Rust reader and writer agree
    // with it byte-for-byte and hash-for-hash.
    let easy_dxf = std::fs::read(golden_dir().join("easy2350.dxf")).expect("easy2350.dxf");
    let easy_json = std::fs::read(golden_dir().join("easy2350.json")).expect("easy2350.json");
    let easy = read(&easy_dxf, &easy_json).expect("read easy2350");
    assert_eq!(easy.outline.elements.len(), 8);
    assert_eq!(easy.thickness_nm(), 1_600_000);
    assert_eq!(hash(&easy), easy.manifest.geometry_hash);
    let (rewritten_dxf, rewritten_json) = write(&easy);
    assert_eq!(rewritten_dxf.as_bytes(), easy_dxf.as_slice());
    assert_eq!(rewritten_json.as_bytes(), easy_json.as_slice());
    let easy_back = read(&rewritten_dxf, &rewritten_json).expect("re-read easy2350");
    assert_eq!(easy_back, easy);
}

#[test]
fn golden_error_files_cover_every_typed_error_variant() {
    let base_round = build_round_board();
    let (valid_dxf, valid_json) = write(&base_round);
    let minimal_header = "  0\nSECTION\n  2\nHEADER\n  9\n$ACADVER\n  1\nAC1009\n  0\nENDSEC\n";

    type ErrorCase<'a> = (&'a str, Vec<u8>, Vec<u8>, fn(&Error) -> bool);
    let cases: Vec<ErrorCase<'_>> = vec![
        (
            "errors/01_invalid_utf8",
            vec![0xff, 0xfe, 0x00, 0x01],
            valid_json.as_bytes().to_vec(),
            |e| matches!(e, Error::InvalidUtf8 { which: "dxf" }),
        ),
        (
            "errors/02_invalid_manifest_json",
            valid_dxf.as_bytes().to_vec(),
            b"{ not valid json }\n".to_vec(),
            |e| matches!(e, Error::InvalidManifestJson { .. }),
        ),
        (
            "errors/03_unsupported_schema",
            valid_dxf.as_bytes().to_vec(),
            valid_json.replace("\"schema\": 1", "\"schema\": 99").into_bytes(),
            |e| matches!(e, Error::UnsupportedSchema { found: 99 }),
        ),
        (
            "errors/04_empty_manifest_field",
            valid_dxf.as_bytes().to_vec(),
            valid_json
                .replace("\"board\": \"round_sensor\"", "\"board\": \"   \"")
                .into_bytes(),
            |e| matches!(e, Error::EmptyManifestField { field: "board" }),
        ),
        (
            "errors/05_invalid_thickness",
            valid_dxf.as_bytes().to_vec(),
            valid_json
                .replace("\"thickness_mm\": 1.6", "\"thickness_mm\": -0.4")
                .into_bytes(),
            |e| matches!(e, Error::InvalidThickness { .. }),
        ),
        (
            "errors/06_dxf_odd_line_count",
            b"  0\nSECTION\n  2\n".to_vec(),
            valid_json.as_bytes().to_vec(),
            |e| matches!(e, Error::DxfOddLineCount),
        ),
        (
            "errors/07_dxf_invalid_group_code",
            b"  0\nSECTION\nXYZ\nENTITIES\n  0\nENDSEC\n  0\nEOF\n".to_vec(),
            valid_json.as_bytes().to_vec(),
            |e| matches!(e, Error::DxfInvalidGroupCode { line: 3, .. }),
        ),
        (
            "errors/08_dxf_invalid_value",
            format!(
                "{minimal_header}  0\nSECTION\n  2\nENTITIES\n  0\nCIRCLE\n  8\n{LAYER_OUTLINE}\n 10\nnot_a_number\n 20\n0.0\n 40\n5.0\n  0\nENDSEC\n  0\nEOF\n"
            )
            .into_bytes(),
            valid_json.as_bytes().to_vec(),
            |e| matches!(e, Error::DxfInvalidValue { code: 10, .. }),
        ),
        (
            "errors/09_dxf_missing_entities",
            format!("{minimal_header}  0\nEOF\n").into_bytes(),
            valid_json.as_bytes().to_vec(),
            |e| matches!(e, Error::DxfMissingEntities),
        ),
        (
            "errors/10_dxf_unclosed_polyline",
            format!(
                "{minimal_header}  0\nSECTION\n  2\nENTITIES\n  0\nPOLYLINE\n  8\n{LAYER_OUTLINE}\n 66\n1\n 70\n1\n  0\nVERTEX\n  8\n{LAYER_OUTLINE}\n 10\n0.0\n 20\n0.0\n  0\nENDSEC\n  0\nEOF\n"
            )
            .into_bytes(),
            valid_json.as_bytes().to_vec(),
            |e| matches!(e, Error::DxfUnclosedPolyline),
        ),
        (
            "errors/11_unsupported_entity",
            format!(
                "{minimal_header}  0\nSECTION\n  2\nENTITIES\n  0\nSPLINE\n  8\n{LAYER_OUTLINE}\n 10\n0.0\n 20\n0.0\n  0\nENDSEC\n  0\nEOF\n"
            )
            .into_bytes(),
            valid_json.as_bytes().to_vec(),
            |e| matches!(e, Error::UnsupportedEntity { kind, .. } if kind == "SPLINE"),
        ),
        (
            "errors/12_degenerate_geometry",
            format!(
                "{minimal_header}  0\nSECTION\n  2\nENTITIES\n  0\nLINE\n  8\n{LAYER_OUTLINE}\n 10\n5.0\n 20\n5.0\n 11\n5.0\n 21\n5.0\n  0\nENDSEC\n  0\nEOF\n"
            )
            .into_bytes(),
            valid_json.as_bytes().to_vec(),
            |e| matches!(e, Error::DegenerateGeometry { .. }),
        ),
        (
            "errors/13_invalid_xdata",
            format!(
                "{minimal_header}  0\nSECTION\n  2\nENTITIES\n  0\nCIRCLE\n  8\n{LAYER_HOLE}\n 10\n5.0\n 20\n5.0\n 40\n1.5\n1001\nBOARD_INTERFACE\n1002\n{{\n1000\nclearance_mm\n1040\n-0.25\n1002\n}}\n  0\nENDSEC\n  0\nEOF\n"
            )
            .into_bytes(),
            valid_json.as_bytes().to_vec(),
            |e| matches!(e, Error::InvalidXdata { .. }),
        ),
        (
            "errors/14_duplicate_id",
            format!(
                "{minimal_header}  0\nSECTION\n  2\nENTITIES\n  0\nCIRCLE\n  8\n{LAYER_OUTLINE}\n 10\n0.0\n 20\n0.0\n 40\n20.0\n1001\nBOARD_INTERFACE\n1000\nid\n1000\nwaffle:dup/1\n  0\nCIRCLE\n  8\n{LAYER_HOLE}\n 10\n0.0\n 20\n0.0\n 40\n1.5\n1001\nBOARD_INTERFACE\n1000\nid\n1000\nwaffle:dup/1\n  0\nENDSEC\n  0\nEOF\n"
            )
            .into_bytes(),
            valid_json.as_bytes().to_vec(),
            |e| matches!(e, Error::DuplicateId { id } if id == "waffle:dup/1"),
        ),
        (
            "errors/15_missing_outline",
            format!(
                "{minimal_header}  0\nSECTION\n  2\nENTITIES\n  0\nCIRCLE\n  8\n{LAYER_HOLE}\n 10\n5.0\n 20\n-5.0\n 40\n1.5\n  0\nENDSEC\n  0\nEOF\n"
            )
            .into_bytes(),
            valid_json.as_bytes().to_vec(),
            |e| matches!(e, Error::MissingOutline),
        ),
        (
            "errors/16_open_loop",
            format!(
                "{minimal_header}  0\nSECTION\n  2\nENTITIES\n  0\nLINE\n  8\n{LAYER_OUTLINE}\n 10\n0.0\n 20\n0.0\n 11\n10.0\n 21\n0.0\n  0\nLINE\n  8\n{LAYER_OUTLINE}\n 10\n10.0\n 20\n0.0\n 11\n10.0\n 21\n-10.0\n  0\nENDSEC\n  0\nEOF\n"
            )
            .into_bytes(),
            valid_json.as_bytes().to_vec(),
            |e| matches!(e, Error::OpenLoop { .. }),
        ),
        (
            "errors/17_branching_loop",
            format!(
                "{minimal_header}  0\nSECTION\n  2\nENTITIES\n\
                  0\nLINE\n  8\n{LAYER_OUTLINE}\n 10\n0.0\n 20\n0.0\n 11\n10.0\n 21\n0.0\n\
                  0\nLINE\n  8\n{LAYER_OUTLINE}\n 10\n10.0\n 20\n0.0\n 11\n10.0\n 21\n-10.0\n\
                  0\nLINE\n  8\n{LAYER_OUTLINE}\n 10\n10.0\n 20\n-10.0\n 11\n0.0\n 21\n0.0\n\
                  0\nLINE\n  8\n{LAYER_OUTLINE}\n 10\n0.0\n 20\n0.0\n 11\n-10.0\n 21\n0.0\n\
                  0\nLINE\n  8\n{LAYER_OUTLINE}\n 10\n-10.0\n 20\n0.0\n 11\n-10.0\n 21\n-10.0\n\
                  0\nLINE\n  8\n{LAYER_OUTLINE}\n 10\n-10.0\n 20\n-10.0\n 11\n0.0\n 21\n0.0\n\
                  0\nENDSEC\n  0\nEOF\n"
            )
            .into_bytes(),
            valid_json.as_bytes().to_vec(),
            |e| matches!(e, Error::BranchingLoop { degree: 4, .. }),
        ),
        (
            "errors/18_multiple_outlines",
            format!(
                "{minimal_header}  0\nSECTION\n  2\nENTITIES\n\
                  0\nCIRCLE\n  8\n{LAYER_OUTLINE}\n 10\n0.0\n 20\n0.0\n 40\n10.0\n\
                  0\nCIRCLE\n  8\n{LAYER_OUTLINE}\n 10\n40.0\n 20\n0.0\n 40\n10.0\n\
                  0\nENDSEC\n  0\nEOF\n"
            )
            .into_bytes(),
            valid_json.as_bytes().to_vec(),
            |e| matches!(e, Error::MultipleOutlines { count: 2 }),
        ),
        (
            "errors/19_cutout_outside_outline",
            format!(
                "{minimal_header}  0\nSECTION\n  2\nENTITIES\n\
                  0\nCIRCLE\n  8\n{LAYER_OUTLINE}\n 10\n0.0\n 20\n0.0\n 40\n10.0\n\
                  0\nCIRCLE\n  8\n{LAYER_CUTOUT}\n 10\n30.0\n 20\n0.0\n 40\n3.0\n\
                  0\nENDSEC\n  0\nEOF\n"
            )
            .into_bytes(),
            valid_json.as_bytes().to_vec(),
            |e| matches!(e, Error::CutoutOutsideOutline),
        ),
        (
            "errors/20_inconsistent_keepout_attributes",
            format!(
                "{minimal_header}  0\nSECTION\n  2\nENTITIES\n\
                  0\nCIRCLE\n  8\n{LAYER_OUTLINE}\n 10\n0.0\n 20\n0.0\n 40\n20.0\n\
                  0\nLINE\n  8\n{LAYER_KEEPOUT}\n 10\n0.0\n 20\n0.0\n 11\n5.0\n 21\n0.0\n\
                1001\nBOARD_INTERFACE\n1000\nloop\n1071\n0\n1000\nside\n1000\nfront\n\
                  0\nLINE\n  8\n{LAYER_KEEPOUT}\n 10\n5.0\n 20\n0.0\n 11\n5.0\n 21\n-5.0\n\
                1001\nBOARD_INTERFACE\n1000\nloop\n1071\n0\n1000\nside\n1000\nback\n\
                  0\nLINE\n  8\n{LAYER_KEEPOUT}\n 10\n5.0\n 20\n-5.0\n 11\n0.0\n 21\n0.0\n\
                1001\nBOARD_INTERFACE\n1000\nloop\n1071\n0\n1000\nside\n1000\nfront\n\
                  0\nENDSEC\n  0\nEOF\n"
            )
            .into_bytes(),
            valid_json.as_bytes().to_vec(),
            |e| matches!(e, Error::InconsistentKeepoutAttributes { loop_index: 0 }),
        ),
    ];

    assert_eq!(
        cases.len(),
        20,
        "all 20 Error variants must have golden files"
    );
    for (stem, dxf_bytes, json_bytes, pred) in cases {
        check_or_update_golden_bytes(&format!("{stem}.dxf"), &dxf_bytes);
        check_or_update_golden_bytes(&format!("{stem}.json"), &json_bytes);
        let dxf_on_disk = std::fs::read(golden_dir().join(format!("{stem}.dxf"))).unwrap();
        let json_on_disk = std::fs::read(golden_dir().join(format!("{stem}.json"))).unwrap();
        let err = read(&dxf_on_disk, &json_on_disk)
            .expect_err(&format!("golden {stem} should fail with a typed error"));
        assert!(
            pred(&err),
            "golden {stem} produced unexpected error variant: {err:?} ({err})"
        );
        // Ensure Display implementation formats cleanly and non-emptily.
        assert!(!err.to_string().is_empty());
    }
}

#[test]
fn awkward_nanometre_values_and_all_quadrant_arcs_round_trip_to_zero_nm() {
    // Huge coordinates (> 500 km + 1 nm, 15+ decimal digits), negative coordinates, odd-nm drills,
    // arcs in all four quadrants and crossing DXF angle 0 in both directions.
    let base_x: i64 = -500_000_000_001_003;
    let base_y: i64 = 750_000_000_002_007;
    let r: i64 = 10_000_000; // 10 mm
    let c = Point::new(base_x, base_y);

    // Build an arc crossing angle 0° in DXF (`start` at -45° Y-down = +45° Y-up, `end` at +45°
    // Y-down = -45° = 315° Y-up):
    // 7_071_068^2 + 7_071_068^2 = 100_000_002_656_224 -> r = 10_000_000.13 nm.
    let d45: i64 = 7_071_068;
    let p_ne = Point::new(c.x + d45, c.y - d45); // Quadrant I in Y-up (+45°)
    let p_se = Point::new(c.x + d45, c.y + d45); // Quadrant IV in Y-up (315°)
    let p_sw = Point::new(c.x - d45, c.y + d45); // Quadrant III in Y-up (225°)
    let p_nw = Point::new(c.x - d45, c.y - d45); // Quadrant II in Y-up (135°)

    // Test both `angle_increasing = true` and `angle_increasing = false` across all four quadrants and across angle 0!
    for angle_increasing in [true, false] {
        // Loop made of one arc crossing angle 0 (`p_ne -> p_se`) plus three straight chords closing
        // around the back (`p_se -> p_sw -> p_nw -> p_ne`), and a cutout with arcs in Q2/Q3/Q4.
        let outline = Loop::new(vec![
            Element::new(
                Some(format!("test:arc-zero-{angle_increasing}")),
                ElementKind::Arc {
                    center: c,
                    start: p_ne,
                    end: p_se,
                    angle_increasing,
                },
            ),
            Element::new(
                Some("test:l1".to_string()),
                ElementKind::Line {
                    start: p_se,
                    end: p_sw,
                },
            ),
            Element::new(
                Some("test:l2".to_string()),
                ElementKind::Line {
                    start: p_sw,
                    end: p_nw,
                },
            ),
            Element::new(
                Some("test:l3".to_string()),
                ElementKind::Line {
                    start: p_nw,
                    end: p_ne,
                },
            ),
        ]);

        // Inside `c`, add four small quadrant arcs on a 2 mm circle (only valid when outline is
        // the small minor arc `angle_increasing = true`, which bulges outward and contains `c`; for `angle_increasing = false`
        // the major 270° arc goes around `c` the other way, so skip cutout when self-intersecting).
        let cutouts = if angle_increasing {
            let d2: i64 = 1_414_214; // ~sqrt(2) mm
            let q1 = Point::new(c.x + d2, c.y - d2);
            let q4 = Point::new(c.x + d2, c.y + d2);
            let q3 = Point::new(c.x - d2, c.y + d2);
            let q2 = Point::new(c.x - d2, c.y - d2);
            vec![Loop::new(vec![
                Element::new(
                    Some("cut:q4".to_string()),
                    ElementKind::Arc {
                        center: c,
                        start: q1,
                        end: q4,
                        angle_increasing: true,
                    },
                ),
                Element::new(
                    Some("cut:q3".to_string()),
                    ElementKind::Arc {
                        center: c,
                        start: q4,
                        end: q3,
                        angle_increasing: true,
                    },
                ),
                Element::new(
                    Some("cut:q2".to_string()),
                    ElementKind::Arc {
                        center: c,
                        start: q3,
                        end: q2,
                        angle_increasing: true,
                    },
                ),
                Element::new(
                    Some("cut:q1".to_string()),
                    ElementKind::Arc {
                        center: c,
                        start: q2,
                        end: q1,
                        angle_increasing: true,
                    },
                ),
            ])]
        } else {
            Vec::new()
        };

        let holes = vec![
            Hole {
                id: Some("hole:odd-1nm".to_string()),
                center: Point::new(c.x - 1, c.y + 1),
                diameter_nm: 1, // literally 1 nm odd diameter!
                plated: false,
                clearance_nm: 3,
            },
            Hole {
                id: Some("hole:odd-large".to_string()),
                center: Point::new(c.x + r - 3, c.y - 7),
                diameter_nm: 3_141_593, // odd nanometre drill
                plated: true,
                clearance_nm: 123_457,
            },
        ];

        let rec = finalize_record(Record {
            outline,
            cutouts,
            holes,
            keepouts: Vec::new(),
            manifest: make_manifest("awkward_nm", Driver::Waffle, 0.800001),
        });

        let (dxf, json) = write(&rec);
        assert_dxf_reals_clean(&dxf);
        let back = read(&dxf, &json).expect("round-trip awkward nm");
        assert_eq!(
            back, rec,
            "angle_increasing={angle_increasing} failed exact nanometre round-trip"
        );
        assert_eq!(hash(&back), hash(&rec));
    }
}

#[test]
fn arc_direction_reverses_properly_across_y_flip_in_written_dxf() {
    let r = 10 * NM_PER_MM;
    // Semicircle from (10, 0) to (-10, 0) around (0, 0), closed by a straight line (-10, 0) -> (10, 0).
    // Case A: `angle_increasing = true` in Y-down passes through (0, +10) in KiCad Y-down, which flips to
    // (0, -10) = 270° in DXF Y-up. Therefore DXF's CCW arc MUST start at 180° (-10, 0) and end at
    // 0° (+10, 0).
    let rec_ccw_ydown = finalize_record(Record {
        outline: Loop::new(vec![
            Element::new(
                Some("waffle:semi/arc".to_string()),
                ElementKind::Arc {
                    center: Point::new(0, 0),
                    start: Point::new(r, 0),
                    end: Point::new(-r, 0),
                    angle_increasing: true,
                },
            ),
            Element::new(
                Some("waffle:semi/chord".to_string()),
                ElementKind::Line {
                    start: Point::new(-r, 0),
                    end: Point::new(r, 0),
                },
            ),
        ]),
        cutouts: Vec::new(),
        holes: Vec::new(),
        keepouts: Vec::new(),
        manifest: make_manifest("flip_a", Driver::Waffle, 1.6),
    });
    let (dxf_a, json_a) = write(&rec_ccw_ydown);
    assert!(
        dxf_a.contains(" 50\n180.000000000\n 51\n0.000000000\n"),
        "Y-down angle_increasing=true semicircle must sweep 180 -> 0 in DXF Y-up:\n{dxf_a}"
    );
    assert_eq!(read(&dxf_a, &json_a).unwrap(), rec_ccw_ydown);

    // Case B: `angle_increasing = false` in Y-down passes through (0, -10) in KiCad Y-down, which flips to
    // (0, +10) = 90° in DXF Y-up. Therefore DXF's CCW arc MUST start at 0° (+10, 0) and end at
    // 180° (-10, 0).
    let rec_cw_ydown = finalize_record(Record {
        outline: Loop::new(vec![
            Element::new(
                Some("waffle:semi/arc".to_string()),
                ElementKind::Arc {
                    center: Point::new(0, 0),
                    start: Point::new(r, 0),
                    end: Point::new(-r, 0),
                    angle_increasing: false,
                },
            ),
            Element::new(
                Some("waffle:semi/chord".to_string()),
                ElementKind::Line {
                    start: Point::new(-r, 0),
                    end: Point::new(r, 0),
                },
            ),
        ]),
        cutouts: Vec::new(),
        holes: Vec::new(),
        keepouts: Vec::new(),
        manifest: make_manifest("flip_b", Driver::Waffle, 1.6),
    });
    let (dxf_b, json_b) = write(&rec_cw_ydown);
    assert!(
        dxf_b.contains(" 50\n0.000000000\n 51\n180.000000000\n"),
        "Y-down angle_increasing=false semicircle must sweep 0 -> 180 in DXF Y-up:\n{dxf_b}"
    );
    assert_eq!(read(&dxf_b, &json_b).unwrap(), rec_cw_ydown);
    assert_ne!(
        hash(&rec_ccw_ydown),
        hash(&rec_cw_ydown),
        "upper vs lower semicircle are distinct geometries and must have distinct hashes"
    );
}

#[test]
fn hash_is_invariant_to_loop_direction_start_point_and_dxf_entity_order() {
    let base = build_rounded_rect();
    let expected_hash = hash(&base);

    // 1. Reverse outline and cutout loops (verifying arcs flip angle_increasing and swap start/end).
    let mut rev = base.clone();
    rev.outline = base.outline.reversed();
    rev.cutouts[0] = base.cutouts[0].reversed();
    rev.keepouts[0].boundary = base.keepouts[0].boundary.reversed();
    assert_eq!(
        hash(&rev),
        expected_hash,
        "reversing loops must preserve hash"
    );

    // 2. Rotate starting element of outline through all 8 positions, in both directions.
    for dir_loop in [&base.outline, &rev.outline] {
        for shift in 0..dir_loop.elements.len() {
            let mut elems = dir_loop.elements[shift..].to_vec();
            elems.extend_from_slice(&dir_loop.elements[..shift]);
            let mut shifted = base.clone();
            shifted.outline = Loop::new(elems);
            assert_eq!(
                hash(&shifted),
                expected_hash,
                "rotating loop start by {shift} must preserve hash"
            );
        }
    }

    // 3. Reverse hole order and shuffle DXF ENTITIES in the serialized DXF file!
    let mut reordered = base.clone();
    reordered.holes.reverse();
    assert_eq!(hash(&reordered), expected_hash);

    let (dxf, json) = write(&base);
    let shuffled_dxf = reverse_dxf_entities(&dxf);
    let parsed_shuffled = read(&shuffled_dxf, &json).expect("read shuffled DXF");
    assert_eq!(hash(&parsed_shuffled), expected_hash);
}

fn assert_dxf_reals_clean(dxf: &str) {
    let lines: Vec<&str> = dxf.lines().collect();
    for pair in lines.chunks_exact(2) {
        let code: i32 = pair[0].trim().parse().expect("valid group code");
        let val = pair[1].trim();
        if (10..=59).contains(&code) || (1040..=1059).contains(&code) {
            assert!(
                !val.contains('e') && !val.contains('E'),
                "DXF real group {code} must never contain exponent: {val}"
            );
            assert_ne!(
                val, "-0.000000000",
                "DXF real group {code} must never be -0.0"
            );
            let frac = val
                .split_once('.')
                .expect("DXF real must have decimal point")
                .1;
            assert_eq!(
                frac.len(),
                9,
                "DXF real group {code} must have 9 fixed decimals: {val}"
            );
        }
    }
}

fn reverse_dxf_entities(dxf: &str) -> String {
    let marker = "  0\nSECTION\n  2\nENTITIES\n";
    let end_marker = "  0\nENDSEC\n  0\nEOF\n";
    let start = dxf.find(marker).unwrap() + marker.len();
    let end = dxf.rfind(end_marker).unwrap();
    let body = &dxf[start..end];

    let lines: Vec<&str> = body.lines().collect();
    let mut chunks: Vec<String> = Vec::new();
    for pair in lines.chunks_exact(2) {
        if pair[0].trim() == "0" {
            chunks.push(String::new());
        }
        let cur = chunks.last_mut().expect("entity starts with code 0");
        cur.push_str(pair[0]);
        cur.push('\n');
        cur.push_str(pair[1]);
        cur.push('\n');
    }
    chunks.reverse();

    let mut out = String::new();
    out.push_str(&dxf[..start]);
    for c in chunks {
        out.push_str(&c);
    }
    out.push_str(&dxf[end..]);
    out
}

#[test]
fn hash_changes_on_any_one_nanometre_real_move_and_within_implements_publish_nothing() {
    let base = build_rounded_rect();
    let base_hash = hash(&base);
    assert!(within(&base, &base, 0));

    // 1. Move one outline corner point (shared by out-top end and out-tr start) by 1 nm in X.
    let mut moved_1nm = base.clone();
    if let ElementKind::Line { end, .. } = &mut moved_1nm.outline.elements[0].kind {
        end.x += 1;
    }
    if let ElementKind::Arc { start, .. } = &mut moved_1nm.outline.elements[1].kind {
        start.x += 1;
    }
    assert_ne!(
        hash(&moved_1nm),
        base_hash,
        "1 nm outline move must change hash"
    );
    assert!(!within(&base, &moved_1nm, 0));
    assert!(
        within(&base, &moved_1nm, 1),
        "1 nm jitter must satisfy within(..., 1)"
    );

    // 2. Move a hole centre by 1 nm in Y.
    let mut hole_1nm = base.clone();
    hole_1nm.holes[0].center.y -= 1;
    assert_ne!(
        hash(&hole_1nm),
        base_hash,
        "1 nm hole move must change hash"
    );
    assert!(!within(&base, &hole_1nm, 0));
    assert!(within(&base, &hole_1nm, 1));

    // 3. Move a hole centre by 2 nm -> exceeds 1 nm publish-nothing tolerance!
    let mut hole_2nm = base.clone();
    hole_2nm.holes[0].center.y -= 2;
    assert!(!within(&base, &hole_2nm, 1));

    // 4. Change hole diameter by 1 nm (odd nanometre).
    let mut drill_1nm = base.clone();
    drill_1nm.holes[0].diameter_nm += 1;
    assert_ne!(
        hash(&drill_1nm),
        base_hash,
        "1 nm drill change must change hash"
    );
    assert!(!within(&base, &drill_1nm, 0));
    assert!(within(&base, &drill_1nm, 1));

    // 5. Change hole clearance by 1 nm, and plated flag.
    let mut clear_1nm = base.clone();
    clear_1nm.holes[0].clearance_nm += 1;
    assert_ne!(hash(&clear_1nm), base_hash);
    assert!(within(&base, &clear_1nm, 1));

    let mut plated_flip = base.clone();
    plated_flip.holes[0].plated = !plated_flip.holes[0].plated;
    assert_ne!(hash(&plated_flip), base_hash);
    assert!(!within(&base, &plated_flip, 1_000_000));

    // 6. Change keepout max_height_nm by 1 nm, and side.
    let mut ko_1nm = base.clone();
    ko_1nm.keepouts[0].max_height_nm = Some(2_000_001);
    assert_ne!(hash(&ko_1nm), base_hash);
    assert!(within(&base, &ko_1nm, 1));

    let mut ko_side = base.clone();
    ko_side.keepouts[0].side = Side::Front;
    assert_ne!(hash(&ko_side), base_hash);
    assert!(!within(&base, &ko_side, 1_000_000));

    // 7. Change board thickness by 1 nm (0.000001 mm).
    let mut thick_1nm = base.clone();
    thick_1nm.manifest.thickness_mm += 0.000_001;
    assert_ne!(
        hash(&thick_1nm),
        base_hash,
        "1 nm thickness change must change hash"
    );
    assert!(!within(&base, &thick_1nm, 0));
    assert!(within(&base, &thick_1nm, 1));
}

#[test]
fn compare_classifies_every_handover_state_and_diffs_per_id() {
    let r1 = build_rounded_rect(); // driver = Composer

    // 1. InSync
    let cmp_same = compare(&r1, &r1);
    assert_eq!(cmp_same.classification, Classification::InSync);
    assert!(cmp_same.changes.is_empty());
    assert_eq!(cmp_same.thickness_changed, None);

    // 2. UpdatedByDriver + per-ID Added, Removed, Moved, Changed + thickness_changed:
    let mut r2 = r1.clone();
    r2.manifest.parent = Some(r1.manifest.revision.clone());
    r2.manifest.thickness_mm = 2.0;
    // Move mh1 by (+1 mm, -0.5 mm), remove mh2, add mh3, change ko-1..4 side to Both.
    r2.holes[0].center.x += NM_PER_MM;
    r2.holes[0].center.y -= 500_000;
    r2.holes.pop();
    r2.holes.push(Hole {
        id: Some("composer:mh3".to_string()),
        center: Point::new(30 * NM_PER_MM, 10 * NM_PER_MM),
        diameter_nm: 2_500_000,
        plated: false,
        clearance_nm: 200_000,
    });
    r2.keepouts[0].side = Side::Both;
    r2.manifest.revision.clear();
    let r2 = finalize_record(r2);

    let cmp_update = compare(&r1, &r2);
    assert_eq!(cmp_update.classification, Classification::UpdatedByDriver);
    assert_eq!(cmp_update.thickness_changed, Some((1_600_000, 2_000_000)));
    assert!(cmp_update.changes.contains(&Change {
        id: Some("composer:mh1".to_string()),
        role: Role::Hole,
        kind: ChangeKind::Moved {
            dx_nm: 1_000_000,
            dy_nm: -500_000,
        },
    }));
    assert!(cmp_update.changes.contains(&Change {
        id: Some("composer:mh2".to_string()),
        role: Role::Hole,
        kind: ChangeKind::Removed,
    }));
    assert!(cmp_update.changes.contains(&Change {
        id: Some("composer:mh3".to_string()),
        role: Role::Hole,
        kind: ChangeKind::Added,
    }));
    assert!(cmp_update.changes.contains(&Change {
        id: Some("composer:ko-1".to_string()),
        role: Role::Keepout,
        kind: ChangeKind::Changed,
    }));

    // 2b. TheirsBehind: when the driver at `r2` checks a follower still at `r1`.
    let cmp_behind = compare(&r2, &r1);
    assert_eq!(cmp_behind.classification, Classification::TheirsBehind);

    // 3. CleanHandover: Waffle Iron takes over from Composer's latest `r2` (`parent = r2.revision`).
    let mut r3_waffle = r2.clone();
    r3_waffle.holes[0].center.x += 2 * NM_PER_MM;
    r3_waffle.manifest.parent = Some(r2.manifest.revision.clone());
    r3_waffle.manifest.driver = Driver::Waffle;
    r3_waffle.manifest.author = Author {
        app: Driver::Waffle,
        source: "fedcba9876543210fedcba9876543210fedcba98".to_string(),
        at: "2026-11-03T13:00:00Z".to_string(),
    };
    r3_waffle.manifest.last_handover = Some(Handover {
        from_revision: r3_waffle.manifest.parent.clone().unwrap(),
        by: Driver::Waffle,
        at: "2026-11-03T13:00:00Z".to_string(),
    });
    r3_waffle.manifest.revision.clear();
    let r3_waffle = finalize_record(r3_waffle);

    let cmp_handover = compare(&r2, &r3_waffle);
    assert_eq!(cmp_handover.classification, Classification::CleanHandover);

    // 3b. A handover the follower missed: the new driver has published again since taking over, so
    // the newest manifest's parent no longer names the follower's revision. Still a clean handover.
    let mut r4_waffle = r3_waffle.clone();
    r4_waffle.manifest.parent = Some(r3_waffle.manifest.revision.clone());
    r4_waffle.manifest.revision = "after-the-handover".to_string();
    assert_eq!(
        compare(&r2, &r4_waffle).classification,
        Classification::CleanHandover,
        "a publish after the take over must not make it look stale"
    );

    // 4. StaleTakeOver: Composer published `r2` from `r1`, while Waffle Iron took over from stale
    // `r1` (never having seen `r2`!).
    let mut stale_waffle = r1.clone();
    stale_waffle.holes[0].diameter_nm = 4_000_000;
    stale_waffle.manifest.parent = Some(r1.manifest.revision.clone());
    stale_waffle.manifest.driver = Driver::Waffle;
    stale_waffle.manifest.author = Author {
        app: Driver::Waffle,
        source: "1111111111111111111111111111111111111111".to_string(),
        at: "2026-11-03T12:30:00Z".to_string(),
    };
    stale_waffle.manifest.last_handover = Some(Handover {
        from_revision: stale_waffle.manifest.parent.clone().unwrap(),
        by: Driver::Waffle,
        at: "2026-11-03T12:30:00Z".to_string(),
    });
    stale_waffle.manifest.revision.clear();
    let stale_waffle = finalize_record(stale_waffle);

    assert_eq!(
        compare(&r2, &stale_waffle).classification,
        Classification::StaleTakeOver,
        "handover from r1 when driver is already at r2 must be StaleTakeOver"
    );
    assert_eq!(
        compare(&stale_waffle, &r2).classification,
        Classification::StaleTakeOver,
        "stale taker seeing driver's newer r2 must also see StaleTakeOver"
    );

    // 5. CompetingTakeOvers: Starting from KiCad-authored `easy2350`, BOTH Waffle Iron and
    // Composer unlock and take over from the same parent revision before seeing each other!
    let easy_dxf = std::fs::read(golden_dir().join("easy2350.dxf")).unwrap();
    let easy_json = std::fs::read(golden_dir().join("easy2350.json")).unwrap();
    let easy_base = read(&easy_dxf, &easy_json).unwrap();

    let mut take_waffle = easy_base.clone();
    take_waffle.holes.push(Hole {
        id: Some("waffle:h1".to_string()),
        center: Point::new(105 * NM_PER_MM, 105 * NM_PER_MM),
        diameter_nm: 2_200_000,
        plated: false,
        clearance_nm: 0,
    });
    take_waffle.manifest.parent = Some(easy_base.manifest.revision.clone());
    take_waffle.manifest.driver = Driver::Waffle;
    take_waffle.manifest.author.app = Driver::Waffle;
    take_waffle.manifest.author.at = "2026-11-03T14:00:00Z".to_string();
    take_waffle.manifest.last_handover = Some(Handover {
        from_revision: take_waffle.manifest.parent.clone().unwrap(),
        by: Driver::Waffle,
        at: "2026-11-03T14:00:00Z".to_string(),
    });
    take_waffle.manifest.revision.clear();
    let take_waffle = finalize_record(take_waffle);

    let mut take_composer = easy_base.clone();
    take_composer.holes.push(Hole {
        id: Some("composer:h1".to_string()),
        center: Point::new(115 * NM_PER_MM, 145 * NM_PER_MM),
        diameter_nm: 3_000_000,
        plated: true,
        clearance_nm: 200_000,
    });
    take_composer.manifest.parent = Some(easy_base.manifest.revision.clone());
    take_composer.manifest.driver = Driver::Composer;
    take_composer.manifest.author.app = Driver::Composer;
    take_composer.manifest.author.at = "2026-11-03T14:01:00Z".to_string();
    take_composer.manifest.last_handover = Some(Handover {
        from_revision: take_composer.manifest.parent.clone().unwrap(),
        by: Driver::Composer,
        at: "2026-11-03T14:01:00Z".to_string(),
    });
    take_composer.manifest.revision.clear();
    let take_composer = finalize_record(take_composer);

    assert_eq!(
        compare(&take_waffle, &take_composer).classification,
        Classification::CompetingTakeOvers
    );
    assert_eq!(
        compare(&take_composer, &take_waffle).classification,
        Classification::CompetingTakeOvers
    );
}

#[test]
fn resaved_dxf_without_xdata_and_polylines_with_bulges_read_and_rematch_or_report_uncertain() {
    let base = build_rounded_rect();
    let (dxf, json) = write(&base);

    // Strip all XDATA (lines with group codes >= 1000) to simulate a CAD re-save that drops XDATA.
    let stripped_dxf = strip_xdata_from_dxf(&dxf);
    let resaved = read(&stripped_dxf, &json).expect("read XDATA-stripped DXF");
    assert!(resaved.outline.elements.iter().all(|e| e.id.is_none()));
    assert!(resaved.holes.iter().all(|h| h.id.is_none()));

    // Unedited re-save: classified ChangedOutsideSuite, all 18 items rematched by geometry, 0 uncertain!
    let cmp_clean = compare(&base, &resaved);
    assert_eq!(
        cmp_clean.classification,
        Classification::ChangedOutsideSuite
    );
    assert!(cmp_clean.uncertain.is_empty(), "{:?}", cmp_clean.uncertain);
    assert_eq!(cmp_clean.rematched.len(), 18);

    // Move BOTH holes in the XDATA-stripped record -> compare MUST report UncertainMatch rather than
    // guessing which hole is mh1 and which is mh2!
    let mut two_holes_moved = resaved.clone();
    two_holes_moved.holes[0].center.x += 2 * NM_PER_MM;
    two_holes_moved.holes[1].center.y -= 2 * NM_PER_MM;
    let cmp_uncertain = compare(&base, &two_holes_moved);
    assert_eq!(
        cmp_uncertain.classification,
        Classification::ChangedOutsideSuite
    );
    assert_eq!(cmp_uncertain.uncertain.len(), 1);
    assert_eq!(cmp_uncertain.uncertain[0].role, Role::Hole);
    assert_eq!(
        cmp_uncertain.uncertain[0].candidate_base_ids,
        vec!["composer:mh1".to_string(), "composer:mh2".to_string()]
    );

    // Test reading LWPOLYLINE with bulges (4 lines + 4 quarter-circle corner arcs of rounded_rect)
    // and R12 POLYLINE with bulges (stadium cutout with two semicircle bulges = -1.0)!
    // Note: in DXF Y-up, rounded_rect `[0, 60] x [-40, 0]` walked CW has `bulge = -(sqrt(2) - 1)`.
    let b_cw_90 = -(2.0_f64.sqrt() - 1.0);
    let poly_dxf = format!(
        "  0\nSECTION\n  2\nHEADER\n  9\n$ACADVER\n  1\nAC1015\n  0\nENDSEC\n\
           0\nSECTION\n  2\nENTITIES\n\
           0\nLWPOLYLINE\n  8\n{LAYER_OUTLINE}\n 90\n8\n 70\n1\n\
          10\n4.0\n 20\n0.0\n\
          10\n56.0\n 20\n0.0\n 42\n{b_cw_90:.12}\n\
          10\n60.0\n 20\n-4.0\n\
          10\n60.0\n 20\n-36.0\n 42\n{b_cw_90:.12}\n\
          10\n56.0\n 20\n-40.0\n\
          10\n4.0\n 20\n-40.0\n 42\n{b_cw_90:.12}\n\
          10\n0.0\n 20\n-36.0\n\
          10\n0.0\n 20\n-4.0\n 42\n{b_cw_90:.12}\n\
           0\nPOLYLINE\n  8\n{LAYER_CUTOUT}\n 66\n1\n 70\n1\n\
           0\nVERTEX\n  8\n{LAYER_CUTOUT}\n 10\n25.0\n 20\n-18.0\n\
           0\nVERTEX\n  8\n{LAYER_CUTOUT}\n 10\n35.0\n 20\n-18.0\n 42\n-1.0\n\
           0\nVERTEX\n  8\n{LAYER_CUTOUT}\n 10\n35.0\n 20\n-22.0\n\
           0\nVERTEX\n  8\n{LAYER_CUTOUT}\n 10\n25.0\n 20\n-22.0\n 42\n-1.0\n\
           0\nSEQEND\n  8\n{LAYER_CUTOUT}\n\
           0\nCIRCLE\n  8\n{LAYER_HOLE}\n 10\n6.0\n 20\n-6.0\n 40\n1.6\n\
           0\nCIRCLE\n  8\n{LAYER_HOLE}\n 10\n54.0\n 20\n-34.0\n 40\n1.6\n\
           0\nLWPOLYLINE\n  8\n{LAYER_KEEPOUT}\n 90\n4\n 70\n1\n\
          10\n10.0\n 20\n-28.0\n\
          10\n20.0\n 20\n-28.0\n\
          10\n20.0\n 20\n-36.0\n\
          10\n10.0\n 20\n-36.0\n\
           0\nENDSEC\n  0\nEOF\n"
    );

    let from_poly = read(&poly_dxf, &json).expect("read LWPOLYLINE and POLYLINE with bulges");
    assert_eq!(from_poly.outline.elements.len(), 8);
    assert_eq!(from_poly.cutouts.len(), 1);
    assert_eq!(from_poly.cutouts[0].elements.len(), 4);
    // Every single element of LWPOLYLINE and POLYLINE matches `base` to within 0 nm, so `compare`
    // rematches all 18 items with 0 geometric changes and 0 uncertain matches!
    let cmp_poly = compare(&base, &from_poly);
    assert_eq!(cmp_poly.classification, Classification::ChangedOutsideSuite);
    assert!(
        cmp_poly.changes.is_empty(),
        "unexpected geometric diff: {:?}",
        cmp_poly.changes
    );
    assert!(
        cmp_poly.uncertain.is_empty(),
        "unexpected uncertain: {:?}",
        cmp_poly.uncertain
    );
    assert_eq!(cmp_poly.rematched.len(), 18);
}

fn strip_xdata_from_dxf(dxf: &str) -> String {
    let mut out = String::new();
    let lines: Vec<&str> = dxf.lines().collect();
    for pair in lines.chunks_exact(2) {
        let code: i32 = pair[0].trim().parse().unwrap();
        if code < 1000 {
            out.push_str(pair[0]);
            out.push('\n');
            out.push_str(pair[1]);
            out.push('\n');
        }
    }
    out
}
