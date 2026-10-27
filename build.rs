fn main() {
    println!("cargo:rerun-if-changed=app.rc");
    println!("cargo:rerun-if-changed=assets/icon.ico");

    // 只在 Windows 目标上编资源；找不到资源编译器时只警告，不让普通构建失败。
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some()
        && let Err(err) = embed_resource::compile("app.rc", embed_resource::NONE).manifest_optional()
    {
        println!("cargo:warning=图标资源未编入 exe（缺少 rc.exe / windres）：{err}");
    }
}
