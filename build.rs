// Icono y datos de versión dentro del .exe. Si no hay `rc.exe` (sin SDK de Windows) el
// programa compila igual, solo que sin icono en el Explorador: la ventana lo lleva aparte.
fn main() {
    println!("cargo:rerun-if-changed=artchat.rc");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if let embed_resource::CompilationResult::Failed(e) =
        embed_resource::compile("artchat.rc", embed_resource::NONE)
    {
        println!("cargo:warning=sin icono en el .exe: {e}");
    }
}
