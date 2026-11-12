//! Local microbenchmarks for Phase 8 inspection contracts.

use koren_inspections::{
    CancellationToken, Finding, FindingOrigin, InspectionDocument, InspectionProfile,
    InspectionProvider, InspectionService, RuleMetadata, Severity,
};
use koren_protocol::{TextPosition, TextRange};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::{Duration, Instant},
};

struct MarkerProvider;

impl InspectionProvider for MarkerProvider {
    fn id(&self) -> &'static str {
        "bench.marker"
    }
    fn rules(&self) -> Vec<RuleMetadata> {
        vec![RuleMetadata {
            id: "bench.marker.todo".into(),
            provider_id: self.id().into(),
            title: "TODO marker".into(),
            description: "Find TODO text".into(),
            category: "style".into(),
            languages: BTreeSet::new(),
            default_severity: Severity::Information,
            supports_fixes: false,
            options: BTreeMap::default(),
        }]
    }
    fn inspect(
        &self,
        document: &InspectionDocument,
        _profile: &InspectionProfile,
        _workspace: Option<&str>,
        cancellation: &CancellationToken,
    ) -> Result<Vec<Finding>, String> {
        let mut findings = Vec::new();
        for (offset, line) in document.text.lines().enumerate() {
            cancellation
                .checkpoint()
                .map_err(|error| error.to_string())?;
            if let Some(column) = line.find("TODO") {
                findings.push(Finding {
                    id: format!("{}:{offset}:{column}", document.path),
                    provider_id: self.id().into(),
                    rule_id: "bench.marker.todo".into(),
                    path: document.path.clone(),
                    document_id: document.document_id,
                    document_revision: document.revision,
                    source_revision: None,
                    range: TextRange {
                        start: TextPosition {
                            line: offset,
                            character: column,
                        },
                        end: TextPosition {
                            line: offset,
                            character: column + 4,
                        },
                    },
                    severity: Severity::Information,
                    message: "TODO marker".into(),
                    explanation: None,
                    category: Some("style".into()),
                    related: Vec::new(),
                    actions: Vec::new(),
                    documentation: None,
                    origin: FindingOrigin::Document,
                });
            }
        }
        Ok(findings)
    }
}

fn measure(label: &str, iterations: u32, mut operation: impl FnMut()) {
    let mut samples = Vec::with_capacity(iterations as usize);
    for _ in 0..iterations {
        let start = Instant::now();
        operation();
        samples.push(start.elapsed());
    }
    let total = samples.iter().copied().sum::<Duration>();
    let mean_ns = total.as_secs_f64() * 1_000_000_000.0 / f64::from(iterations);
    let minimum_ns = samples.iter().min().copied().unwrap_or_default().as_nanos();
    println!("{label},iterations={iterations},mean_ns={mean_ns:.1},min_ns={minimum_ns}");
}

fn main() {
    let mut service = InspectionService::with_concurrency(2);
    service
        .register(Arc::new(MarkerProvider))
        .expect("register benchmark provider");
    let profile = InspectionProfile::new("Default");
    let document = InspectionDocument {
        path: "src/main.txt".into(),
        document_id: Some(1),
        revision: Some(1),
        language: "text".into(),
        text: Arc::from("TODO marker\n".repeat(200)),
    };
    let rule = service.rules().remove(0);
    measure("profile_lookup", 100_000, || {
        std::hint::black_box(profile.effective_rule(None, &rule));
    });
    measure("diagnostic_ingestion_200_lines", 500, || {
        let job = service
            .inspect_document(document.clone(), profile.clone(), None)
            .expect("start bounded inspection");
        std::hint::black_box(
            job.recv_timeout(Duration::from_secs(1))
                .expect("receive findings"),
        );
    });
}
