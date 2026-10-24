extern crate winres;

fn main() {
    build_hidapi();

    let mut res = winres::WindowsResource::new();
    res.set_icon("src/icons/main.ico");
    res.set("FileDescription", "Battery Status");
    res.set("ProductName", "Battery Status");
    res.set("InternalName", "BatteryStatus");
    res.set("OriginalFilename", "BatteryStatus.exe");

    // Application manifest for dark mode support
    res.set_manifest(r#"
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0" xmlns:asmv3="urn:schemas-microsoft-com:asm.v3">
    <asmv3:application>
        <asmv3:windowsSettings>
            <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true</dpiAware>
            <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
            <activeCodePage xmlns="http://schemas.microsoft.com/SMI/2019/WindowsSettings">UTF-8</activeCodePage>
        </asmv3:windowsSettings>
    </asmv3:application>
    <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
        <application>
            <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
        </application>
    </compatibility>
    <dependency>
        <dependentAssembly>
            <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
        </dependentAssembly>
    </dependency>
</assembly>
"#);

    res.compile().unwrap();
}

/// Compile the vendored Windows hidapi backend used by `native_devices`.
fn build_hidapi() {
    println!("cargo:rerun-if-changed=vendor/hidapi/windows/hid.c");
    cc::Build::new()
        .file("vendor/hidapi/windows/hid.c")
        .include("vendor/hidapi/hidapi")
        .include("vendor/hidapi/windows")
        .warnings(false)
        .compile("hidapi");
    println!("cargo:rustc-link-lib=setupapi");
}
