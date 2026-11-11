use std::{env, process::Command};

fn main() {
    let frontend = env::var("CARGO_MANIFEST_DIR").expect("manifest directory") + "/frontend";
    println!("cargo:rerun-if-changed={frontend}/package.json");
    println!("cargo:rerun-if-changed={frontend}/package-lock.json");
    println!("cargo:rerun-if-changed={frontend}/src");
    println!("cargo:rerun-if-changed={frontend}/index.html");
    println!("cargo:rerun-if-changed={frontend}/tsconfig.json");
    println!("cargo:rerun-if-changed={frontend}/vite.config.ts");

    let status = Command::new("npm")
        .arg("run")
        .arg("build")
        .current_dir(&frontend)
        .status()
        .expect(
            "npm is required to build Koren Code Editor; run npm ci in apps/koren-code/frontend",
        );
    if !status.success() {
        panic!("frontend build failed; run npm ci in apps/koren-code/frontend");
    }
}
