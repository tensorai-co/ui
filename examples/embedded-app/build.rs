fn main() {
    // A real app would add `.before("npm run build")`; this example ships a prebuilt `dist/`.
    ui_build::web("web/dist").emit();
}
