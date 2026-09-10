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
fn malformed_size_fails_in_xml_and_does_not_create_a_clr_rom() {
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
    let parsed = parse_dat(clr.as_slice()).unwrap();
    assert!(parsed.games[0].roms.is_empty());
}
