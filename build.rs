// Incrusta el ícono y los datos de la app en el .exe (solo Windows).
fn main() {
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("fysh.ico");
        res.set("FileDescription", "fysh");
        res.set("ProductName", "fysh");
        res.compile().expect("no se pudo incrustar el ícono");
    }
}
