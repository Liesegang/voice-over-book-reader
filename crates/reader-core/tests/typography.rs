use reader_core::{CssLength, parse_html};

fn resolved(size: Option<CssLength>) -> f32 {
    size.unwrap().resolve(24., 24., 24.)
}

#[test]
fn real_user_agent_stylesheet_supplies_heading_size_weight_and_logical_spacing() {
    let chapter = parse_html(
        "<body><h1>大見出し</h1><h2>小見出し</h2><p>本文</p></body>",
        0,
        "typography.xhtml",
        "",
    )
    .unwrap();
    assert!((resolved(chapter.blocks[0].font_size) - 33.6).abs() < 0.001);
    assert!((resolved(chapter.blocks[1].font_size) - 30.).abs() < 0.001);
    assert_eq!(chapter.blocks[0].margin_block_end, Some(CssLength::Em(0.8)));
    assert_eq!(
        chapter.blocks[1].margin_block_end,
        Some(CssLength::Em(0.65))
    );
    assert!(chapter.blocks[0].atoms.iter().all(|atom| atom.style.bold));
    assert_eq!(
        reader_core::default_reader_css_revision(),
        blake3::hash(reader_core::DEFAULT_READER_CSS.as_bytes())
            .to_hex()
            .as_str()
    );
}

#[test]
fn author_origin_beats_more_specific_reader_defaults_and_can_remove_heading_gap_and_bold() {
    let chapter = parse_html(
        "<body><h1>著者の見出し</h1></body>",
        0,
        "author.xhtml",
        "* { font-size: 100%; font-weight: normal; margin: 0; }",
    )
    .unwrap();
    let block = &chapter.blocks[0];
    assert_eq!(resolved(block.font_size), 24.);
    assert_eq!(block.margin_block_start, Some(CssLength::Px(0.)));
    assert_eq!(block.margin_block_end, Some(CssLength::Px(0.)));
    assert!(block.atoms.iter().all(|atom| !atom.style.bold));
    let chapter = parse_html(
        "<h1 style='font-size:150%'>優先順位</h1>",
        0,
        "priority.xhtml",
        "h1 { font-size: 200% !IMPORTANT; }",
    )
    .unwrap();
    assert_eq!(resolved(chapter.blocks[0].font_size), 48.);
}

#[test]
fn inherited_em_percent_absolute_and_root_units_are_computed_once_before_layout() {
    let chapter = parse_html("<body style='font-size:1.2em'><div style='font-size:150%'><h2 style='font-size:2em'>甲<span style='font-size:50%'>乙</span><span style='font-size:2rem'>丙</span></h2></div><p style='font-size:32px'>丁<span style='font-size:150%'>戊</span></p></body>", 0, "inherit.xhtml", "").unwrap();
    let atoms = chapter.atoms();
    for (text, size) in [
        ("甲", 86.4),
        ("乙", 43.2),
        ("丙", 48.),
        ("丁", 32.),
        ("戊", 48.),
    ] {
        let atom = atoms.iter().find(|atom| atom.text == text).unwrap();
        assert!(
            (resolved(atom.style.font_size) - size).abs() < 0.001,
            "{text}"
        );
    }
}

#[test]
fn logical_and_physical_margin_shorthands_follow_writing_mode_and_cascade_order() {
    let chapter = parse_html("<body><h2 style='writing-mode:vertical-rl; margin:1em 2em 3em 4em; margin-block-end:5em'>縦の章</h2><h2 style='margin-block:2em 3em; margin:0'>横の章</h2><p>本文</p></body>", 0, "margins.xhtml", "").unwrap();
    assert_eq!(
        chapter.blocks[0].margin_block_start,
        Some(CssLength::Em(2.))
    );
    assert_eq!(chapter.blocks[0].margin_block_end, Some(CssLength::Em(5.)));
    assert_eq!(chapter.blocks[1].margin_block_end, Some(CssLength::Px(0.)));
    assert_eq!(
        chapter.blocks[2].margin_block_end,
        Some(CssLength::Px(0.)),
        "margins are not inherited from a previous block"
    );
    let inherited = parse_html("<body style='writing-mode:vertical-rl'><p style='writing-mode:inherit; margin-right:2em; margin-left:3em'>継承した縦組</p></body>",0,"inherited-mode.xhtml","").unwrap();
    assert_eq!(
        inherited.blocks[0].margin_block_start,
        Some(CssLength::Em(2.))
    );
    assert_eq!(
        inherited.blocks[0].margin_block_end,
        Some(CssLength::Em(3.))
    );
}

#[test]
fn typography_does_not_change_original_text_or_source_locators() {
    let html = "<h2>題<span>名</span></h2><p>原文<ruby>東京<rt>とうきょう</rt></ruby>を読む。</p>";
    let original = parse_html(html, 4, "source.xhtml", "").unwrap();
    let styled = parse_html(
        html,
        4,
        "source.xhtml",
        "h2 { font-size:3em; margin-block:2em; } p { font-size:90%; }",
    )
    .unwrap();
    assert_eq!(original.plain_text(), styled.plain_text());
    for (a, b) in original.atoms().iter().zip(styled.atoms()) {
        assert_eq!(a.sources, b.sources);
        assert_eq!(a.ruby, b.ruby);
    }
}

#[test]
fn invalid_author_font_and_margin_values_do_not_discard_valid_reader_defaults() {
    for value in ["nonsense", "-2em", "1.5", "calc(1em + 2px)"] {
        let chapter = parse_html(
            "<body><h2>見出し</h2></body>",
            0,
            "invalid.xhtml",
            &format!("h2 {{ font-size:{value}; margin-block-end:nonsense; }}"),
        )
        .unwrap();
        assert_eq!(resolved(chapter.blocks[0].font_size), 30.);
        assert_eq!(
            chapter.blocks[0].margin_block_end,
            Some(CssLength::Em(0.65))
        );
    }
}
