//! UI-contribution, Lua callback, and ownership-registry microbenchmarks.

use koren_extensions::{
    CommandContext, ContributionBuilder, ContributionRegistry, ExtensionManifest,
};
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

fn manifest() -> ExtensionManifest {
    ExtensionManifest::parse(
        "schema_version=1\nid='org.example.phase7'\nname='Phase 7 Bench'\npublisher='Koren'\nversion='0.1.0'\ndescription='Benchmark'\nentry_point='main.lua'\napi_version='1.0.0'\ncapabilities=['commands','ui_contributions','settings','themes','diagnostics']\n",
    ).expect("valid benchmark manifest")
}

fn source() -> &'static str {
    r##"
      local m = {}
      function m.activate()
        for i = 1, 64 do
          koren.commands.register("command-" .. i, "Command " .. i, "Benchmark command", function() return "ok" end)
        end
        koren.commands.register("refresh", "Refresh", "Update panel", function()
          koren.ui.update_tool_window("panel", "Updated workspace contents")
          return "ok"
        end)
        koren.ui.register_tool_window("panel", "Benchmark Panel", "Initial contents")
        koren.settings.register_boolean("enabled", "Enabled", true)
        koren.themes.register("benchmark", "Benchmark", "dark", { accent = "#123456" })
        koren.diagnostics.provide_detail("detail", nil, nil, "Detail", "Benchmark explanation")
      end
      return m
    "##
}

#[cfg(target_os = "linux")]
fn rss_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        line.strip_prefix("VmRSS:")?
            .split_whitespace()
            .next()?
            .parse()
            .ok()
    })
}

fn main() {
    let manifest = manifest();
    measure("contribution_registry_init", 100_000, || {
        std::hint::black_box(ContributionRegistry::new());
    });

    measure(
        "lua_activate_64_commands_panel_setting_theme_diagnostic",
        100,
        || {
            let mut builder = ContributionBuilder::new(&manifest);
            let runtime = LuaExtensionRuntime::activate_source(
                &manifest,
                source(),
                LuaHostContext::default(),
                LuaLimits::default(),
                &mut builder,
            )
            .expect("activate");
            let mut registry = ContributionRegistry::new();
            registry
                .register(&manifest.id, builder.into_contributions())
                .expect("register contributions");
            runtime.deactivate().expect("deactivate");
            std::hint::black_box(registry);
        },
    );

    let mut builder = ContributionBuilder::new(&manifest);
    let runtime = LuaExtensionRuntime::activate_source(
        &manifest,
        source(),
        LuaHostContext::default(),
        LuaLimits::default(),
        &mut builder,
    )
    .expect("activate benchmark runtime");
    let contributions = builder.into_contributions();
    let mut registry = ContributionRegistry::new();
    registry
        .register(&manifest.id, contributions.clone())
        .expect("register benchmark contributions");
    println!("registered_commands={}", contributions.commands.len());
    println!(
        "lua_vm_memory_after_activation_bytes={}",
        runtime.used_memory().expect("memory")
    );
    measure("lua_tool_window_update_and_drain", 10_000, || {
        runtime
            .call_command("refresh", &CommandContext::default(), &[])
            .expect("update panel");
        std::hint::black_box(runtime.take_ui_updates().expect("drain updates"));
    });
    registry.unregister(&manifest.id);
    runtime.deactivate().expect("deactivate runtime");

    #[cfg(target_os = "linux")]
    {
        let before = rss_kib().unwrap_or_default();
        let mut builder = ContributionBuilder::new(&manifest);
        let runtime = LuaExtensionRuntime::activate_source(
            &manifest,
            source(),
            LuaHostContext::default(),
            LuaLimits::default(),
            &mut builder,
        )
        .expect("activate memory fixture");
        let during = rss_kib().unwrap_or_default();
        println!("process_rss_without_active_extension_kib={before}");
        println!("process_rss_with_one_active_extension_kib={during}");
        println!("process_rss_delta_kib={}", during.saturating_sub(before));
        runtime.deactivate().expect("deactivate memory fixture");
    }
}
