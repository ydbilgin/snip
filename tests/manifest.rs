// A named GetWindowSubclass import requires the v6 activation context at process load.
#[test]
fn manifest_declares_v6_common_controls_for_native_text_subclass() {
    let manifest = include_str!("../snip.manifest");
    let dependency = manifest.split("<dependency>").skip(1).find(|s| s.contains("name=\"Microsoft.Windows.Common-Controls\""))
        .expect("native text input statically imports GetWindowSubclass, absent from system32 v5 comctl32");
    let end = dependency.find("</dependency>").expect("closed dependency");
    let dependency = &dependency[..end];
    for required in ["<dependentAssembly>", "version=\"6.0.0.0\"", "publicKeyToken=\"6595b64144ccf1df\"", "processorArchitecture=\"*\"", "language=\"*\""] {
        assert!(dependency.contains(required), "missing v6 assembly identity: {required}");
    }
}
