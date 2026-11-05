use super::*;

#[test]
fn truncate_keeps_grapheme_clusters_whole() {
    let family = "👨‍👩‍👧";
    let cut = truncate(&format!("{family}{family}"), 3);

    assert_eq!(cut, format!("{family}…"));
    assert!(truncate("❤️❤️❤️❤️", 4).width() <= 4);
}
