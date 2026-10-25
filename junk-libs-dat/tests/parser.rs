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
        "<datafile><game name=\"No bytes\"/></datafile>",
        "<datafile><game name=\"Disk\"><disk name=\"chd\"/></game></datafile>",
        "<datafile><machine name=\"Machine\"/></datafile>",
        "<datafile><game name=\"Nested\"><game name=\"Other\"/></game></datafile>",
    ] {
        assert!(parse_dat(xml.as_bytes()).is_err(), "{xml}");
    }
    let xml = format!(
        "<datafile><header><name>{}</name></header></datafile>",
        "x".repeat(65537)
    );
    assert!(parse_dat(xml.as_bytes()).is_err());
    let clr = format!("clrmamepro (\nname \"{}\"\n)\n", "x".repeat(65537));
    assert!(parse_dat(clr.as_bytes()).is_err());
}

#[test]
fn clr_cannot_hide_unsupported_members_beside_valid_roms() {
    let clr=b"clrmamepro (\nname \"GB\"\nversion 1\n)\ngame (\nname \"Game\"\nrom ( name \"game.gb\" size 3 )\ndisk ( name \"hidden\" )\n)\n";
    assert!(parse_dat(clr.as_slice()).is_err());
}

#[test]
fn xml_text_entities_and_cdata_preserve_complete_values() {
    let xml = r#"<datafile><header>
        <name>  Games &amp; More &#x65E5;&#26412;  </name>
        <description>one &lt; two &gt; zero &quot;quoted&quot; &apos;apostrophe&apos;</description>
        <author>A &amp;amp; B</author><url>https://example.test/?a=1&amp;b=2</url>
        <version>1<![CDATA[ & 2]]> &amp; 3</version>
        </header><game name="Title &amp; More">
        <serial>AB &amp; CD</serial><version>1 &lt; 2</version><category> Games &amp; Demos </category>
        <rom name="日本 &amp; More.bin" size="4" crc="AABBCCDD"/>
        </game></datafile>"#;
    // Exercise the public streaming parser, including very small reader buffers.
    for capacity in [1, 8192] {
        let dat = parse_dat(std::io::BufReader::with_capacity(capacity, xml.as_bytes())).unwrap();
        assert_eq!(dat.name, "Games & More 日本");
        assert_eq!(dat.description, "one < two > zero \"quoted\" 'apostrophe'");
        assert_eq!(dat.author, "A &amp; B");
        assert_eq!(dat.url, "https://example.test/?a=1&b=2");
        assert_eq!(dat.version, "1 & 2 & 3");
        let game = &dat.games[0];
        assert_eq!(game.name, "Title & More");
        assert_eq!(game.serial.as_deref(), Some("AB & CD"));
        assert_eq!(game.roms[0].serial, game.serial);
        assert_eq!(game.version.as_deref(), Some("1 < 2"));
        assert_eq!(game.category.as_deref(), Some("Games & Demos"));
        assert_eq!(game.roms[0].name, "日本 & More.bin");
    }
}

#[test]
fn invalid_entities_and_dtd_entities_fail_instead_of_losing_text() {
    for value in ["&unknown;", "&#x110000;", "&#0;", "&#nope;", "&amp"] {
        let xml =
            format!("<datafile><header><name>before {value} after</name></header></datafile>");
        assert!(parse_dat(xml.as_bytes()).is_err(), "{value}");
    }
    let xml = br#"<!DOCTYPE datafile [<!ENTITY title "expanded">]><datafile><header><name>&title;</name></header></datafile>"#;
    assert!(parse_dat(xml.as_slice()).is_err());
}

#[test]
fn many_attributes_keep_duplicate_rejection_and_token_bounds() {
    let attributes = (0..3000)
        .map(|i| format!(" a{i}=\"x\""))
        .collect::<String>();
    let xml = format!(
        "<datafile><game name=\"Game\"{attributes}><rom name=\"one\" size=\"1\"/></game></datafile>"
    );
    assert_eq!(parse_dat(xml.as_bytes()).unwrap().games[0].roms.len(), 1);
    let duplicate_game = xml.replace(
        "<game name=\"Game\"",
        "<game name=\"Game\" a2999=\"duplicate\"",
    );
    assert!(parse_dat(duplicate_game.as_bytes()).is_err());
    let duplicate_rom = format!(
        "<datafile><game name=\"Game\"><rom name=\"one\" size=\"1\"{attributes} a2999=\"duplicate\"/></game></datafile>"
    );
    assert!(parse_dat(duplicate_rom.as_bytes()).is_err());
    let oversized = format!(
        "<datafile><game name=\"{}\"><rom name=\"one\" size=\"1\"/></game></datafile>",
        "x".repeat(65537)
    );
    assert!(parse_dat(oversized.as_bytes()).is_err());
}

#[test]
fn split_xml_text_cannot_bypass_the_value_bound() {
    let xml = format!(
        "<datafile><header><name>{}</name></header></datafile>",
        "<![CDATA[x]]>".repeat(65537)
    );
    assert!(parse_dat(xml.as_bytes()).is_err());
}

#[test]
fn delimiters_inside_xml_events_cannot_bypass_the_read_bound() {
    let padding = ">".repeat(65537);
    let attribute = format!(
        "<datafile><header><name>System</name></header><game name=\"Game\" padding=\"{padding}\"><rom name=\"one\" size=\"1\"/></game></datafile>"
    );
    assert!(parse_dat(attribute.as_bytes()).is_err());

    // Comments are ignored by the DAT consumer, so a post-event value-size
    // check cannot protect the XML parser's allocation here.
    let comment = format!(
        "<datafile><header><name>System</name></header><!--{}--></datafile>",
        "<>".repeat(65537)
    );
    assert!(parse_dat(comment.as_bytes()).is_err());
    let cdata = format!(
        "<datafile><header><name><![CDATA[{}]]></name></header></datafile>",
        "<>".repeat(65537)
    );
    assert!(parse_dat(cdata.as_bytes()).is_err());
}
