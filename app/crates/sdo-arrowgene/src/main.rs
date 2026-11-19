#[cfg(not(target_arch = "wasm32"))]
fn main() -> anyhow::Result<()> {
    sdo_arrowgene::run()
}
#[cfg(target_arch = "wasm32")]
fn main() {}
