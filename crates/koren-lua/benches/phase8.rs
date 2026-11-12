//! Bounded Lua inspection callback benchmark for Phase 8.

use koren_extensions::{ContributionBuilder, ExtensionManifest};
use koren_inspections::{
    CancellationToken, InspectionDocument, InspectionProfile, InspectionProvider,
};
use koren_lua::{LuaExtensionRuntime, LuaHostContext, LuaLimits};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

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
        "schema_version=1\nid='org.example.phase8-bench'\nname='Phase 8 Bench'\npublisher='Koren'\nversion='0.1.0'\ndescription='Benchmark'\nentry_point='main.lua'\napi_version='1.0.0'\ncapabilities=['diagnostics','document_read']\n",
    ).expect("valid benchmark manifest");
    let source = r#"
      local m = {}
      function m.activate()
        koren.inspections.register_rule({id="todo", title="TODO", description="Find TODO markers", category="style", languages={"plaintext"}, severity="warning"}, function(document, settings)
          local findings = {}
          if settings.enabled then
            local start_at, end_at = document.text:find("TODO", 1, true)
            while start_at do
              findings[#findings + 1] = {start_line=0, start_column=start_at-1, end_line=0, end_column=end_at, message="TODO marker", replacement="DONE"}
              start_at, end_at = document.text:find("TODO", end_at+1, true)
            end
          end
          return findings
        end)
      end
      return m
    "#;
    let mut builder = ContributionBuilder::new(&manifest);
    let runtime = LuaExtensionRuntime::activate_source(
        &manifest,
        source,
        LuaHostContext::default(),
        LuaLimits::default(),
        &mut builder,
    )
    .expect("activate benchmark extension");
    let document = InspectionDocument {
        path: "src/fixture.txt".into(),
        document_id: Some(1),
        revision: Some(1),
        language: "plaintext".into(),
        text: Arc::from("TODO marker ".repeat(200)),
    };
    let profile = InspectionProfile::new("Default");
    measure(
        "lua_inspection_callback_200_markers_single_line",
        500,
        || {
            let findings = runtime
                .inspect(&document, &profile, None, &CancellationToken::new())
                .expect("inspect fixture");
            std::hint::black_box(findings);
        },
    );
    runtime
        .deactivate()
        .expect("deactivate benchmark extension");
}
