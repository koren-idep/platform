//! Extension manager and Lua lifecycle microbenchmarks.

use koren_extensions::{ExtensionManager, ExtensionManifest};
use koren_lua::{LuaExtensionRuntime, LuaHostContext, LuaLimits};
use std::time::{Duration, Instant};

fn measure(label: &str, iterations: u32, mut operation: impl FnMut()) {
    let mut samples = Vec::with_capacity(iterations as usize);
    for _ in 0..iterations {
        let start = Instant::now();
        operation();
        samples.push(start.elapsed());
    }
    let mean = samples.iter().map(Duration::as_secs_f64).sum::<f64>() * 1e6 / f64::from(iterations);
    let min = samples
        .iter()
        .copied()
        .min()
        .unwrap_or_default()
        .as_secs_f64()
        * 1e6;
    println!("{label},iterations={iterations},mean_us={mean:.3},min_us={min:.3}");
}

fn main() {
    let manifest = ExtensionManifest::parse(
        "schema_version=1\nid='org.example.bench'\nname='Bench'\npublisher='Koren'\nversion='0.1.0'\ndescription='Benchmark'\nentry_point='main.lua'\napi_version='1.0.0'\ncapabilities=['commands']\n",
    ).expect("valid benchmark manifest");
    let source = r#"local m={}; function m.activate() koren.commands.register("bench", "Bench", "Bench callback", function() return "ok" end) end; return m"#;

    measure("extension_manager_init", 10_000, || {
        std::hint::black_box(ExtensionManager::new());
    });
    let mut memory = 0usize;
    measure("lua_init_activate_and_deactivate", 500, || {
        let mut contributions = koren_extensions::ContributionBuilder::new(&manifest);
        let runtime = LuaExtensionRuntime::activate_source(
            &manifest,
            source,
            LuaHostContext::default(),
            LuaLimits::default(),
            &mut contributions,
        )
        .expect("activate benchmark extension");
        memory = runtime.used_memory().expect("read Lua memory");
        runtime
            .deactivate()
            .expect("deactivate benchmark extension");
        std::hint::black_box(contributions.into_contributions());
    });
    println!("lua_vm_memory_after_activation_bytes={memory}");
}
