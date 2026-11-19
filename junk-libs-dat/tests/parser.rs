use junk_libs_dat::parse_dat;

#[test]
fn logiqx_and_clrmamepro_produce_the_same_neutral_record() {
    let xml = br#"<datafile><header><name>System</name><version>1</version></header><game name="Title &amp; More"><serial>ABCD</serial><rom name="Title &amp; More.bin" size="4" crc="AABBCCDD" sha1="0011"/></game></datafile>"#;
    let clr = br#"clrmamepro (
 name "System"
 version 1
)
game (
 name "Title & More"
 serial "ABCD"
 rom ( name "Title & More.bin" size 4 crc AABBCCDD sha1 0011 )
)
"#;
    let xml = parse_dat(xml.as_slice()).unwrap();
    let clr = parse_dat(clr.as_slice()).unwrap();
    assert_eq!(xml.name, clr.name);
    assert_eq!(xml.games[0].name, clr.games[0].name);
    assert_eq!(xml.games[0].roms[0].crc, "aabbccdd");
    assert_eq!(xml.games[0].roms[0].serial.as_deref(), Some("ABCD"));
    assert_eq!(xml.games[0].roms[0].name, clr.games[0].roms[0].name);
}

#[test]
fn malformed_size_fails_in_both_formats() {
    let xml = br#"<datafile><header><name>System</name></header><game name="Bad"><rom name="bad.bin" size="NaN" crc="00"/></game></datafile>"#;
    assert!(parse_dat(xml.as_slice()).is_err());
    let clr = br#"clrmamepro (
 name "System"
)
game (
 name "Bad"
 rom ( name "bad.bin" size NaN crc 00 )
)
"#;
    assert!(parse_dat(clr.as_slice()).is_err());
}

#[test]
fn malformed_catalogs_cannot_silently_drop_files_or_trailing_games() {
    use std::io::Cursor;
    let xml = r#"<datafile><header><name>Test</name><version>1</version></header><game name="one"><rom name="one" size="1" crc="01"/></game></datafile>"#;
    assert!(junk_libs_dat::parse_dat(Cursor::new(xml.trim_end_matches("</datafile>"))).is_err());
    assert!(junk_libs_dat::parse_dat(Cursor::new(xml.replace("size=\"1\"", ""))).is_err());
    let text = "clrmamepro (\n name Test\n version 1\n)\ngame (\n name one\n rom ( name one size 1 crc 01 )\n rom ( name two size BAD crc 02 )\n)\n";
    assert!(junk_libs_dat::parse_dat(Cursor::new(text)).is_err());
    assert!(junk_libs_dat::parse_dat(Cursor::new(text.replace("size BAD", ""))).is_err());
    let valid = text.replace("size BAD", "size 2");
    assert!(junk_libs_dat::parse_dat(Cursor::new(&valid)).is_ok());
    assert!(
        junk_libs_dat::parse_dat(Cursor::new(format!("{valid}game (\n name truncated\n"))).is_err()
    );
    assert!(junk_libs_dat::parse_dat(Cursor::new(valid.replace("crc 02 )", "crc 02"))).is_err());
}

#[test]
fn header_metadata_and_bom_are_content_detected() {
    let xml=b"\xef\xbb\xbf  <datafile><header><name>PSX</name><author>Redump</author><url>http://redump.org</url><version>20261010</version></header><game name=\"Game\"><rom name=\"track.bin\" size=\"2352\"/></game></datafile>";
    let dat = parse_dat(xml.as_slice()).unwrap();
    assert_eq!(dat.dialect, junk_libs_dat::DatDialect::LogiqxXml);
    assert_eq!(dat.author, "Redump");
    assert_eq!(dat.url, "http://redump.org");
    let clr=b"  clrmamepro (\nname \"GB\"\nauthor \"No-Intro\"\nversion \"1\"\n)\ngame (\n name \"Game\"\n rom ( name \"game.gb\" size 3 )\n)\n";
    let dat = parse_dat(clr.as_slice()).unwrap();
    assert_eq!(dat.dialect, junk_libs_dat::DatDialect::ClrMamePro);
    assert_eq!(dat.author, "No-Intro");
}

#[test]
fn unsupported_entries_and_oversized_tokens_do_not_vanish() {
    for xml in [
        "<datafile><game name=\"Disk\"><disk name=\"chd\"/></game></datafile>",
        "<datafile><machine name=\"Machine\"/></datafile>",
        "<datafile><game name=\"Nested\"><game name=\"Other\"/></game></datafile>",
    ] {
        assert!(parse_dat(xml.as_bytes()).is_err(), "{xml}");
    }
    let xml = format!(
        "<datafile><header><name>{}</name></header></datafile>",
        "x".repeat(1 << 20)
    );
    assert!(parse_dat(xml.as_bytes()).is_err());
    let clr = format!("clrmamepro (\nname \"{}\"\n)\n", "x".repeat(1 << 20));
    assert!(parse_dat(clr.as_bytes()).is_err());
}

#[test]
fn clr_cannot_hide_unsupported_members_beside_valid_roms() {
    let clr=b"clrmamepro (\nname \"GB\"\nversion 1\n)\ngame (\nname \"Game\"\nrom ( name \"game.gb\" size 3 )\ndisk ( name \"hidden\" )\n)\n";
    assert!(parse_dat(clr.as_slice()).is_err());
}

#[test]
fn games_without_roms_are_kept() {
    for xml in [
        "<datafile><game name=\"Empty\"/></datafile>",
        "<datafile><game name=\"Empty\"></game></datafile>",
    ] {
        let dat = parse_dat(xml.as_bytes()).unwrap();
        assert_eq!(dat.games.len(), 1, "{xml}");
        assert_eq!(dat.games[0].name, "Empty");
        assert!(dat.games[0].roms.is_empty());
    }
}

#[test]
fn text_entities_and_cdata_join_into_one_value() {
    let xml = r#"<datafile><header>
        <name>  Games &amp; More &#x65E5;&#26412;  </name>
        <version>1<![CDATA[ & 2]]> &amp; 3</version>
        </header><game name="Title &amp; More">
        <serial>AB &amp; CD</serial>
        <rom name="a.bin" size="4" crc="AABBCCDD"/>
        </game></datafile>"#;
    for capacity in [1, 8192] {
        let dat = parse_dat(std::io::BufReader::with_capacity(capacity, xml.as_bytes())).unwrap();
        assert_eq!(dat.name, "Games & More 日本");
        assert_eq!(dat.version, "1 & 2 & 3");
        assert_eq!(dat.games[0].name, "Title & More");
        assert_eq!(dat.games[0].serial.as_deref(), Some("AB & CD"));
    }
}

#[test]
fn unknown_and_dtd_entities_fail() {
    for value in ["&unknown;", "&#x110000;", "&amp"] {
        let xml = format!("<datafile><header><name>{value}</name></header></datafile>");
        assert!(parse_dat(xml.as_bytes()).is_err(), "{value}");
    }
    let xml = br#"<!DOCTYPE datafile [<!ENTITY t "x">]><datafile><header><name>&t;</name></header></datafile>"#;
    assert!(parse_dat(xml.as_slice()).is_err());
}

#[test]
fn delimiters_inside_an_event_do_not_reset_the_read_bound() {
    let docs = |n: usize| {
        [
            format!(
                "<datafile><header><name>{}</name></header><game name=\"g\" description=\"{}\"/></datafile>",
                "x",
                ">".repeat(n)
            ),
            format!(
                "<datafile><header><name>x</name></header><!--{}--></datafile>",
                "<>".repeat(n)
            ),
            format!(
                "<datafile><header><name><![CDATA[{}]]></name></header></datafile>",
                "<>".repeat(n)
            ),
        ]
    };
    for xml in docs(10) {
        parse_dat(xml.as_bytes()).unwrap();
    }
    for xml in docs(65537) {
        assert!(parse_dat(xml.as_bytes()).is_err());
    }
}
