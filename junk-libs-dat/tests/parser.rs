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
