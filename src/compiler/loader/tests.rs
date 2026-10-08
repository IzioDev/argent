use super::*;
use crate::compiler::model::AppCompilationContext;

#[test]
fn selected_apps_reject_duplicate_actor_exports_instead_of_renaming_them() {
    let temp = temp_dir("duplicate-selected-exports");
    fs::write(temp.join("left.ag"), "state Left {} actor A owns Left {}").unwrap();
    fs::write(temp.join("right.ag"), "state Right {} actor A owns Right {}").unwrap();
    fs::write(
        temp.join("root.ag"),
        r#"
        import "./left.ag" as left;
        import "./right.ag" as right;
        app Test { actor left::A; actor right::A; }
    "#,
    )
    .unwrap();
    let program = load_program(temp.join("root.ag")).expect("module namespaces are distinct");
    let app = program.root_app(Some("Test")).expect("test app resolves").expect("test app exists");
    let error = program.app_actor_ids(app).expect_err("app exports must be unambiguous");
    assert!(error.to_string().contains("selected app exports actor name `A` more than once"), "{error}");
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn resolves_transitive_namespaced_reexports() {
    let temp = temp_dir("namespaced-reexport");
    fs::write(temp.join("leaf.ag"), "state Stored {}\n").expect("leaf source written");
    fs::write(temp.join("middle.ag"), "import \"./leaf.ag\" as shared;\n").expect("middle source written");
    fs::write(
        temp.join("root.ag"),
        r#"
import "./middle.ag" as assets;

state Stored {}

state Wrapper {
    assets::shared::Stored value;
}
"#,
    )
    .expect("root source written");

    let program = load_program(temp.join("root.ag")).expect("module graph loads");
    let root = program.root_module();
    let wrapper = root.states.iter().find(|state| state.name == "Wrapper").expect("wrapper state is loaded");
    assert_eq!(wrapper.fields[0].ty.name, "assets::shared::Stored");

    let model = AppCompilationContext::from_resolved(
        &program,
        None,
        &std::collections::BTreeMap::new(),
        &crate::compiler::model::default_route_planner,
    )
    .expect("resolved declarations build the compiler model");
    let imported_name = model
        .states
        .keys()
        .find(|name| name.ends_with("__Stored"))
        .expect("the namespaced state receives a collision-safe internal name");
    let wrapper_id = model.types.names["Wrapper"];
    let imported_id = match program.bindings(wrapper_id).names.get("assets::shared::Stored") {
        Some(crate::compiler::resolve::ResolvedName::Declaration(id)) => *id,
        _ => panic!("namespaced field type has a bound declaration"),
    };
    assert_eq!(model.types.display_names[&imported_id], *imported_name);
    assert!(model.states.contains_key("Stored"), "the root declaration keeps its source name");

    let _ = fs::remove_dir_all(temp);
}

#[test]
fn rejects_ambiguous_open_reexports() {
    let temp = temp_dir("ambiguous-open-reexport");
    fs::write(temp.join("left.ag"), "const int LIMIT = 1;\n").expect("left source written");
    fs::write(temp.join("right.ag"), "const int LIMIT = 2;\n").expect("right source written");
    fs::write(temp.join("root.ag"), "import \"./left.ag\";\nimport \"./right.ag\";\n").expect("root source written");

    let err = load_program(temp.join("root.ag")).expect_err("ambiguous open re-export is rejected");
    assert!(err.to_string().contains("ambiguous export `LIMIT` in module namespace"), "unexpected error: {err}");

    let _ = fs::remove_dir_all(temp);
}

#[test]
fn app_graph_orders_and_deduplicates_diamond_dependencies() {
    let temp = temp_dir("diamond");
    write_app(&temp.join("shared.ag"), "", "SharedApp", "Shared", &[]);
    write_app(&temp.join("left.ag"), "import \"./shared.ag\" as shared;", "LeftApp", "Left", &["shared::SharedApp::Shared"]);
    write_app(&temp.join("right.ag"), "import \"./shared.ag\" as shared;", "RightApp", "Right", &["shared::SharedApp::Shared"]);
    write_app(
        &temp.join("root.ag"),
        "import \"./left.ag\" as left;\nimport \"./right.ag\" as right;",
        "RootApp",
        "Root",
        &["left::LeftApp::Left", "right::RightApp::Right"],
    );

    let graph = load_app_graph(temp.join("root.ag"), "RootApp").expect("app graph loads");
    assert_eq!(
        graph.units().iter().map(|unit| unit.source_app.app.as_str()).collect::<Vec<_>>(),
        ["SharedApp", "LeftApp", "RightApp", "RootApp"]
    );
    assert_eq!(graph.units().iter().filter(|unit| unit.source_app.app == "SharedApp").count(), 1);
    let root = graph.units().last().unwrap();
    assert_eq!(root.dependencies.iter().map(|dependency| dependency.app.as_str()).collect::<Vec<_>>(), ["LeftApp", "RightApp"]);
    assert_eq!(graph.program_for(root).module_paths().count(), 4, "module imports retain the complete source graph");
    let left = graph.program_for(&graph.units()[1]);
    let right = graph.program_for(&graph.units()[2]);
    assert_eq!(
        left.module_paths().map(|path| path.file_name().unwrap().to_string_lossy().into_owned()).collect::<Vec<_>>(),
        ["left.ag", "shared.ag"],
    );
    assert_eq!(
        right.module_paths().map(|path| path.file_name().unwrap().to_string_lossy().into_owned()).collect::<Vec<_>>(),
        ["right.ag", "shared.ag"],
    );

    let _ = fs::remove_dir_all(temp);
}

#[test]
fn inline_display_label_cannot_shadow_a_standard_source_identity() {
    let program = load_inline_program(PathBuf::from("std::core"), "import \"std::core\";".to_string())
        .expect("inline and standard sources have distinct identities");
    assert_eq!(program.module_paths().count(), 2);
}

#[test]
fn discovery_finds_imports_after_other_top_level_declarations() {
    let program = load_inline_program(
        PathBuf::from("late-import.ag"),
        "const int VALUE = 1; fn helper() { int import = VALUE; } import \"std::core\";".to_string(),
    )
    .expect("late standard import is discovered and parsed");
    assert_eq!(program.module_paths().count(), 2);
}

#[test]
fn app_graph_infers_apps_declared_by_module_imports() {
    let temp = temp_dir("module-app");
    write_app(&temp.join("asset.ag"), "", "AssetApp", "Asset", &[]);
    fs::write(
        temp.join("controller.ag"),
        r#"
import "./asset.ag";

state ControllerState {}

actor Controller owns ControllerState {
    entry inspect(cov_id asset_id)
    observes asset by asset_id {
        inputs {
            src: AssetApp::Asset,
        }
    }
    emits none {}
}

app ControllerApp {
    actor Controller;
}
"#,
    )
    .expect("controller source written");

    let graph = load_app_graph(temp.join("controller.ag"), "ControllerApp").expect("module app dependency graph loads");
    assert_eq!(graph.units().iter().map(|unit| unit.source_app.app.as_str()).collect::<Vec<_>>(), ["AssetApp", "ControllerApp"]);
    let root = graph.units().last().unwrap();
    assert_eq!(root.dependencies.iter().map(|dependency| dependency.app.as_str()).collect::<Vec<_>>(), ["AssetApp"]);
    assert_eq!(graph.program_for(root).module_paths().count(), 2, "ordinary imports retain shared source declarations");

    let _ = fs::remove_dir_all(temp);
}

#[test]
fn module_apps_without_qualified_references_remain_shared_source() {
    let temp = temp_dir("shared-module-app");
    write_app(&temp.join("shared.ag"), "", "SharedApp", "Shared", &[]);
    write_app(&temp.join("root.ag"), "import \"./shared.ag\";", "RootApp", "Root", &[]);

    let graph = load_app_graph(temp.join("root.ag"), "RootApp").expect("shared source module loads");
    assert_eq!(graph.units().iter().map(|unit| unit.source_app.app.as_str()).collect::<Vec<_>>(), ["RootApp"]);
    assert!(graph.units()[0].dependencies.is_empty());
    assert_eq!(graph.program_for(&graph.units()[0]).module_paths().count(), 2);

    let _ = fs::remove_dir_all(temp);
}

#[test]
fn app_graph_reports_the_complete_cycle() {
    let temp = temp_dir("cycle");
    write_app(&temp.join("a.ag"), "import \"./b.ag\" as b;", "AApp", "A", &["b::BApp::B"]);
    write_app(&temp.join("b.ag"), "import \"./c.ag\" as c;", "BApp", "B", &["c::CApp::C"]);
    write_app(&temp.join("c.ag"), "import \"./a.ag\" as a;", "CApp", "C", &["a::AApp::A"]);

    let err = load_app_graph(temp.join("a.ag"), "AApp").expect_err("app cycle is rejected");
    assert!(err.to_string().contains("app import cycle: AApp -> BApp -> CApp -> AApp"), "unexpected error: {err}");

    let _ = fs::remove_dir_all(temp);
}

#[test]
fn app_graph_rejects_one_namespace_from_two_sources() {
    let temp = temp_dir("namespace-collision");
    write_app(&temp.join("first.ag"), "", "AssetApp", "First", &[]);
    write_app(&temp.join("second.ag"), "", "AssetApp", "Second", &[]);
    write_app(
        &temp.join("controller.ag"),
        "import \"./first.ag\" as first;\nimport \"./second.ag\" as second;",
        "CtrlApp",
        "Ctrl",
        &["first::AssetApp::First", "second::AssetApp::Second"],
    );

    let err = load_app_graph(temp.join("controller.ag"), "CtrlApp").expect_err("ambiguous app namespace is rejected");
    assert!(err.to_string().contains("app `AssetApp` is imported from both"), "unexpected error: {err}");

    let _ = fs::remove_dir_all(temp);
}

#[test]
fn app_graph_rejects_namespace_conflicts_across_branches() {
    let temp = temp_dir("transitive-namespace-collision");
    write_app(&temp.join("first.ag"), "", "SharedApp", "First", &[]);
    write_app(&temp.join("second.ag"), "", "SharedApp", "Second", &[]);
    write_app(&temp.join("left.ag"), "import \"./first.ag\" as shared;", "LeftApp", "Left", &["shared::SharedApp::First"]);
    write_app(&temp.join("right.ag"), "import \"./second.ag\" as shared;", "RightApp", "Right", &["shared::SharedApp::Second"]);
    write_app(
        &temp.join("root.ag"),
        "import \"./left.ag\" as left;\nimport \"./right.ag\" as right;",
        "RootApp",
        "Root",
        &["left::LeftApp::Left", "right::RightApp::Right"],
    );

    let err = load_app_graph(temp.join("root.ag"), "RootApp").expect_err("one app namespace must have one source");
    assert!(err.to_string().contains("app `SharedApp` is imported from both"), "unexpected error: {err}");

    let _ = fs::remove_dir_all(temp);
}

fn temp_dir(name: &str) -> PathBuf {
    let temp = std::env::temp_dir().join(format!("argent-app-graph-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp);
    fs::create_dir_all(&temp).expect("temp directory is created");
    temp
}

fn write_app(path: &Path, imports: &str, app: &str, actor: &str, dependencies: &[&str]) {
    let entries = dependencies
        .iter()
        .enumerate()
        .map(|(index, dependency)| {
            format!(
                r#"
    entry dependency_{index}(cov_id dependency_id)
    observes dependency by dependency_id {{
        inputs {{
            source: {dependency},
        }}
    }}
    emits none {{}}
"#
            )
        })
        .collect::<String>();
    fs::write(
        path,
        format!(
            r#"
{imports}

state {actor}State {{}}

actor {actor} owns {actor}State {{
{entries}
}}

app {app} {{
    actor {actor};
}}
"#
        ),
    )
    .expect("app source is written");
}
