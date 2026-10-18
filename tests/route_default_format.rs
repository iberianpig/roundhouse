#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn route_default_formats_match_rails_in_emitted_helpers() {
    let routes = include_str!("route_default_format_routes.rb");
    let model = include_str!("route_default_format_model.rb");
    let oracle_model = model
        .replace("pwa_manifest_path", "RailsRouteHelpers.pwa_manifest_path")
        .replace("plain_path", "RailsRouteHelpers.plain_path")
        .replace(
            "string_manifest_path",
            "RailsRouteHelpers.string_manifest_path",
        )
        .replace("scoped_path", "RailsRouteHelpers.scoped_path");
    let oracle = format!(
        "gem \"railties\", \"8.1.4\"\ngem \"actionpack\", \"8.1.4\"\nrequire \"rails\"\nrequire \"action_controller/railtie\"\nroutes = ActionDispatch::Routing::RouteSet.new\nroutes.draw do\n{routes}end\nRailsRouteHelpers = routes.url_helpers\n{oracle_model}\np FormatRouteProbe.paths\n"
    );
    let expected = std::process::Command::new("ruby")
        .arg("-e")
        .arg(oracle)
        .output()
        .expect("Rails route oracle");
    assert!(
        expected.status.success(),
        "{}",
        String::from_utf8_lossy(&expected.stderr)
    );
    let expected = String::from_utf8(expected.stdout).unwrap();
    emit_and_run::empty_app()
        .write("config/routes.rb", &format!("Rails.application.routes.draw do\n{routes}end\n"))
        .write("app/services/format_route_probe.rb", model)
        .run_ruby(&format!("expected = {}\nactual = FormatRouteProbe.paths\nraise actual.inspect unless actual == expected\nputs \"Route format defaults match Rails\"\n", expected.trim()))
        .assert_passes();
}
