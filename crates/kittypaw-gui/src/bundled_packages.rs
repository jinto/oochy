use std::path::Path;

struct BundledPackage {
    id: &'static str,
    package_toml: &'static str,
    main_js: &'static str,
    readme: &'static str,
}

const BUNDLED_PACKAGES: &[BundledPackage] = &[
    BundledPackage {
        id: "macro-economy-report",
        package_toml: include_str!("../../../packages/macro-economy-report/package.toml"),
        main_js: include_str!("../../../packages/macro-economy-report/main.js"),
        readme: include_str!("../../../packages/macro-economy-report/README.md"),
    },
    BundledPackage {
        id: "weather-briefing",
        package_toml: include_str!("../../../packages/weather-briefing/package.toml"),
        main_js: include_str!("../../../packages/weather-briefing/main.js"),
        readme: include_str!("../../../packages/weather-briefing/README.md"),
    },
    BundledPackage {
        id: "rss-digest",
        package_toml: include_str!("../../../packages/rss-digest/package.toml"),
        main_js: include_str!("../../../packages/rss-digest/main.js"),
        readme: include_str!("../../../packages/rss-digest/README.md"),
    },
    BundledPackage {
        id: "reminder",
        package_toml: include_str!("../../../packages/reminder/package.toml"),
        main_js: include_str!("../../../packages/reminder/main.js"),
        readme: include_str!("../../../packages/reminder/README.md"),
    },
    BundledPackage {
        id: "url-monitor",
        package_toml: include_str!("../../../packages/url-monitor/package.toml"),
        main_js: include_str!("../../../packages/url-monitor/main.js"),
        readme: include_str!("../../../packages/url-monitor/README.md"),
    },
    BundledPackage {
        id: "air-quality",
        package_toml: include_str!("../../../packages/air-quality/package.toml"),
        main_js: include_str!("../../../packages/air-quality/main.js"),
        readme: include_str!("../../../packages/air-quality/README.md"),
    },
    BundledPackage {
        id: "delivery-tracker",
        package_toml: include_str!("../../../packages/delivery-tracker/package.toml"),
        main_js: include_str!("../../../packages/delivery-tracker/main.js"),
        readme: include_str!("../../../packages/delivery-tracker/README.md"),
    },
    BundledPackage {
        id: "lotto-check",
        package_toml: include_str!("../../../packages/lotto-check/package.toml"),
        main_js: include_str!("../../../packages/lotto-check/main.js"),
        readme: include_str!("../../../packages/lotto-check/README.md"),
    },
    BundledPackage {
        id: "news-digest-kr",
        package_toml: include_str!("../../../packages/news-digest-kr/package.toml"),
        main_js: include_str!("../../../packages/news-digest-kr/main.js"),
        readme: include_str!("../../../packages/news-digest-kr/README.md"),
    },
    BundledPackage {
        id: "stock-alert",
        package_toml: include_str!("../../../packages/stock-alert/package.toml"),
        main_js: include_str!("../../../packages/stock-alert/main.js"),
        readme: include_str!("../../../packages/stock-alert/README.md"),
    },
];

pub fn install_bundled_packages(packages_dir: &Path) {
    for pkg in BUNDLED_PACKAGES {
        let pkg_dir = packages_dir.join(pkg.id);
        if pkg_dir.exists() {
            continue; // Already installed
        }
        if let Err(e) = std::fs::create_dir_all(&pkg_dir) {
            tracing::error!("Failed to create package dir {}: {e}", pkg.id);
            continue;
        }
        if let Err(e) = std::fs::write(pkg_dir.join("package.toml"), pkg.package_toml) {
            tracing::error!("Failed to write package.toml for {}: {e}", pkg.id);
            continue;
        }
        if let Err(e) = std::fs::write(pkg_dir.join("main.js"), pkg.main_js) {
            tracing::error!("Failed to write main.js for {}: {e}", pkg.id);
            continue;
        }
        if let Err(e) = std::fs::write(pkg_dir.join("README.md"), pkg.readme) {
            tracing::error!("Failed to write README.md for {}: {e}", pkg.id);
            continue;
        }
        tracing::info!("Installed bundled package: {}", pkg.id);
    }
}
