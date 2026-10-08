use crate::compiler::loader::load_app_graph;
use std::fs;

#[test]
fn dependency_root_views_share_parsed_declarations() {
    let temp = std::env::temp_dir().join(format!("argent-shared-app-graph-{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp);
    fs::create_dir_all(&temp).expect("temp directory");
    fs::write(temp.join("dep.ag"), "state DepState {} actor Dep owns DepState {} app DepApp { actor Dep; }").expect("dependency");
    fs::write(
        temp.join("root.ag"),
        r#"import "./dep.ag" as dep;

            state RootState {}
            actor Root owns RootState {
                entry inspect(cov_id dependency_id)
                observes dependency by dependency_id {
                    inputs { source: dep::DepApp::Dep, }
                }
                emits none {}
            }
            app RootApp { actor Root; }
            "#,
    )
    .expect("root");

    let plan = load_app_graph(temp.join("root.ag"), "RootApp").expect("app graph");
    let [dependency, root] = plan.units() else { panic!("dependency and root units expected") };
    let dependency_view = plan.program_for(dependency);
    let root_view = plan.program_for(root);
    assert_eq!(dependency_view.module_paths().count(), 1);
    assert_eq!(root_view.module_paths().count(), 2);
    let dependency_module = dependency_view.root_module();
    let same_module_in_root = root_view.module(dependency.root);
    assert!(std::ptr::eq(dependency_module, same_module_in_root), "both views retain one parsed declaration tree");

    let _ = fs::remove_dir_all(temp);
}
