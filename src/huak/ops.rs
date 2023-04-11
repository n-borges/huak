use crate::{
    dependency::{dependency_iter, Dependency},
    environment::{env_path_values, Environment},
    error::{Error, HuakResult},
    fs, git,
    metadata::LocalMetadata,
    metadata::{default_entrypoint_string, default_test_file_contents},
    package::importable_package_name,
    python_environment::InstallOptions,
    sys, Config, PythonEnvironment, WorkspaceOptions,
};

const DEFAULT_PYTHON_INIT_FILE_CONTENTS: &str = r#"__version__ = "0.0.1"
"#;
const DEFAULT_PYTHON_MAIN_FILE_CONTENTS: &str = r#"def main():
    print("Hello, World!")


if __name__ == "__main__":
    main()
"#;

///! This module implements various operations to interact with valid workspaces
///! existing on a system.
///
use std::{env::consts::OS, path::Path, process::Command, str::FromStr};
use termcolor::Color;

pub struct AddOptions {
    pub install_options: InstallOptions,
}

pub struct BuildOptions {
    /// A values vector of build options typically used for passing on arguments.
    pub values: Option<Vec<String>>,
    pub install_options: InstallOptions,
}

pub struct FormatOptions {
    /// A values vector of format options typically used for passing on arguments.
    pub values: Option<Vec<String>>,
    pub install_options: InstallOptions,
}

pub struct LintOptions {
    /// A values vector of lint options typically used for passing on arguments.
    pub values: Option<Vec<String>>,
    pub include_types: bool,
    pub install_options: InstallOptions,
}

pub struct RemoveOptions {
    pub install_options: InstallOptions,
}

pub struct PublishOptions {
    /// A values vector of publish options typically used for passing on arguments.
    pub values: Option<Vec<String>>,
    pub install_options: InstallOptions,
}

pub struct TestOptions {
    /// A values vector of test options typically used for passing on arguments.
    pub values: Option<Vec<String>>,
    pub install_options: InstallOptions,
}

pub struct UpdateOptions {
    pub install_options: InstallOptions,
}

pub struct CleanOptions {
    pub include_pycache: bool,
    pub include_compiled_bytecode: bool,
}

pub fn activate_python_environment(config: &Config) -> HuakResult<()> {
    let workspace = config.workspace();
    let python_env = workspace.current_python_environment()?;

    if python_env.active() {
        return Ok(());
    }

    #[cfg(unix)]
    let mut cmd = Command::new("bash");
    #[cfg(unix)]
    cmd.args([
        "--init-file",
        &format!(
            "{}",
            python_env.executables_dir_path().join("activate").display()
        ),
        "-i",
    ]);
    #[cfg(windows)]
    let mut cmd = Command::new("powershell");
    #[cfg(windows)]
    cmd.args([
        "-executionpolicy",
        "bypass",
        "-NoExit",
        "-NoLogo",
        "-File",
        &format!(
            "{}",
            python_env
                .executables_dir_path()
                .join("activate.ps1")
                .display()
        ),
    ]);

    config.terminal().run_command(&mut cmd)
}

pub fn add_project_dependencies(
    dependencies: &[String],
    config: &Config,
    options: &AddOptions,
) -> HuakResult<()> {
    let workspace = config.workspace();
    let package = workspace.current_package()?;
    let mut metadata = workspace.current_local_metadata()?;

    // Collect all dependencies that need to be added to the metadata file.
    let deps = dependency_iter(dependencies)
        .filter(|dep| {
            !metadata
                .metadata()
                .contains_dependency(dep)
                .unwrap_or_default()
        })
        .collect::<Vec<_>>();

    if deps.is_empty() {
        return Ok(());
    }

    let python_env = workspace.resolve_python_environment()?;
    python_env.install_packages(&deps, &options.install_options, config)?;

    // If there's no version data then get the installed version and add to metadata file.
    for pkg in python_env.installed_packages()?.iter().filter(|pkg| {
        deps.iter().any(|dep| {
            pkg.name() == dep.name()
                && dep.requirement().version_or_url.is_none()
        })
    }) {
        let dep = Dependency::from_str(&pkg.to_string())?;
        metadata.metadata_mut().add_dependency(dep);
    }

    // Whatever hasn't been added, add as-is.
    for dep in deps {
        if !metadata.metadata().contains_dependency(&dep)? {
            metadata.metadata_mut().add_dependency(dep);
        }
    }

    if package.metadata() != metadata.metadata() {
        metadata.write_file()?;
    }

    Ok(())
}

pub fn add_project_optional_dependencies(
    dependencies: &[String],
    group: &str,
    config: &Config,
    options: &AddOptions,
) -> HuakResult<()> {
    let workspace = config.workspace();
    let package = workspace.current_package()?;
    let mut metadata = workspace.current_local_metadata()?;

    // Collect all dependencies that need to be added.
    let deps = dependency_iter(dependencies)
        .filter(|dep| {
            !metadata
                .metadata()
                .contains_optional_dependency(dep, group)
                .unwrap_or_default()
        })
        .collect::<Vec<Dependency>>();

    if deps.is_empty() {
        return Ok(());
    };

    let python_env = workspace.resolve_python_environment()?;
    python_env.install_packages(&deps, &options.install_options, config)?;

    // If there's no version data then get the installed version and add to metadata file.
    for pkg in python_env.installed_packages()?.iter().filter(|pkg| {
        deps.iter().any(|dep| {
            pkg.name() == dep.name()
                && dep.requirement().version_or_url.is_none()
        })
    }) {
        let dep = Dependency::from_str(&pkg.to_string())?;
        metadata.metadata_mut().add_optional_dependency(dep, group);
    }

    // Add whatever else as-is.
    for dep in deps {
        if !metadata
            .metadata()
            .contains_optional_dependency(&dep, group)?
        {
            metadata.metadata_mut().add_optional_dependency(dep, group);
        }
    }

    if package.metadata() != metadata.metadata() {
        metadata.write_file()?;
    }

    Ok(())
}

pub fn build_project(
    config: &Config,
    options: &BuildOptions,
) -> HuakResult<()> {
    let workspace = config.workspace();
    let package = workspace.current_package()?;
    let mut metadata = workspace.current_local_metadata()?;
    let python_env = workspace.resolve_python_environment()?;

    // Install the `build` package if it isn't already installed.
    let build_dep = Dependency::from_str("build")?;
    if !python_env.contains_module(build_dep.name())? {
        python_env.install_packages(
            &[&build_dep],
            &options.install_options,
            config,
        )?;
    }

    // Add the installed `build` package to the metadata file.
    if !metadata.metadata().contains_dependency_any(&build_dep)? {
        for pkg in python_env
            .installed_packages()?
            .iter()
            .filter(|pkg| pkg.name() == build_dep.name())
        {
            metadata.metadata_mut().add_optional_dependency(
                Dependency::from_str(&pkg.to_string())?,
                "dev",
            );
        }
    }

    if package.metadata() != metadata.metadata() {
        metadata.write_file()?;
    }

    // Run `build`.
    let mut cmd = Command::new(python_env.python_path());
    let mut args = vec!["-m", "build"];
    if let Some(it) = options.values.as_ref() {
        args.extend(it.iter().map(|item| item.as_str()));
    }
    make_venv_command(&mut cmd, &python_env)?;
    cmd.args(args).current_dir(workspace.root());

    config.terminal().run_command(&mut cmd)
}

pub fn clean_project(
    config: &Config,
    options: &CleanOptions,
) -> HuakResult<()> {
    let workspace = config.workspace();

    // Remove everything from the dist directory if it exists.
    if workspace.root().join("dist").exists() {
        std::fs::read_dir(workspace.root().join("dist"))?
            .filter_map(|x| x.ok().map(|item| item.path()))
            .for_each(|item| {
                if item.is_dir() {
                    std::fs::remove_dir_all(item).ok();
                } else if item.is_file() {
                    std::fs::remove_file(item).ok();
                }
            });
    }

    // Remove all __pycache__ directories in the workspace if they exist.
    if options.include_pycache {
        let pattern = format!(
            "{}",
            workspace.root().join("**").join("__pycache__").display()
        );
        glob::glob(&pattern)?.for_each(|item| {
            if let Ok(it) = item {
                std::fs::remove_dir_all(it).ok();
            }
        })
    }

    // Remove all .pyc files in the workspace if they exist.
    if options.include_compiled_bytecode {
        let pattern =
            format!("{}", workspace.root().join("**").join("*.pyc").display());
        glob::glob(&pattern)?.for_each(|item| {
            if let Ok(it) = item {
                std::fs::remove_file(it).ok();
            }
        })
    }

    Ok(())
}

pub fn format_project(
    config: &Config,
    options: &FormatOptions,
) -> HuakResult<()> {
    let workspace = config.workspace();
    let package = workspace.current_package()?;
    let mut metadata = workspace.current_local_metadata()?;
    let python_env = workspace.resolve_python_environment()?;

    // Install `ruff` and `black` if they aren't already installed.
    let format_deps = [
        Dependency::from_str("black")?,
        Dependency::from_str("ruff")?,
    ];

    let new_format_deps = format_deps
        .iter()
        .filter(|dep| {
            !python_env.contains_module(dep.name()).unwrap_or_default()
        })
        .collect::<Vec<_>>();

    if !new_format_deps.is_empty() {
        python_env.install_packages(
            &new_format_deps,
            &options.install_options,
            config,
        )?;
    }

    // Add the installed `ruff` and `black` packages to the metadata file if not already there.
    let new_format_deps = format_deps
        .iter()
        .filter(|dep| {
            !metadata
                .metadata()
                .contains_dependency_any(dep)
                .unwrap_or_default()
        })
        .map(|dep| dep.name())
        .collect::<Vec<_>>();

    if !new_format_deps.is_empty() {
        for pkg in python_env
            .installed_packages()?
            .iter()
            .filter(|pkg| new_format_deps.contains(&pkg.name()))
        {
            metadata.metadata_mut().add_optional_dependency(
                Dependency::from_str(&pkg.to_string())?,
                "dev",
            );
        }
    }

    if package.metadata() != metadata.metadata() {
        metadata.write_file()?;
    }

    // Run `ruff` and `black` for formatting imports and the rest of the Python code in the workspace.
    let mut terminal = config.terminal();
    let mut cmd = Command::new(python_env.python_path());
    let mut ruff_cmd = Command::new(python_env.python_path());
    let mut ruff_args =
        vec!["-m", "ruff", "check", ".", "--select", "I001", "--fix"];
    make_venv_command(&mut cmd, &python_env)?;
    make_venv_command(&mut ruff_cmd, &python_env)?;
    let mut args = vec!["-m", "black", "."];
    if let Some(v) = options.values.as_ref() {
        args.extend(v.iter().map(|item| item.as_str()));
        if v.contains(&"--check".to_string()) {
            terminal.print_warning(
                    "this check will exit early if imports aren't sorted (see https://github.com/cnpryer/huak/issues/510)",
                )?;
            ruff_args.retain(|item| *item != "--fix")
        }
    }
    ruff_cmd.args(ruff_args).current_dir(workspace.root());
    terminal.run_command(&mut ruff_cmd)?;
    cmd.args(args).current_dir(workspace.root());
    terminal.run_command(&mut cmd)
}

pub fn init_app_project(
    config: &Config,
    options: &WorkspaceOptions,
) -> HuakResult<()> {
    init_lib_project(config, options)?;

    let workspace = config.workspace();
    let mut metadata = workspace.current_local_metadata()?;

    let as_dep = Dependency::from_str(metadata.metadata().project_name())?;
    let entry_point = default_entrypoint_string(
        importable_package_name(as_dep.name())?.as_str(),
    );
    metadata
        .metadata_mut()
        .add_script(as_dep.name(), &entry_point);

    metadata.write_file()
}

pub fn init_lib_project(
    config: &Config,
    options: &WorkspaceOptions,
) -> HuakResult<()> {
    let workspace = config.workspace();

    // Create a metadata file or error if one already exists.
    let mut metadata = match workspace.current_local_metadata() {
        Ok(_) => return Err(Error::MetadataFileFound),
        Err(_) => {
            LocalMetadata::template(workspace.root().join("pyproject.toml"))
        }
    };

    if options.uses_git {
        init_git(&config.workspace_root)?;
    }

    let name = fs::last_path_component(&config.workspace_root)?;
    metadata.metadata_mut().set_project_name(name);
    metadata.write_file()
}

pub fn install_project_dependencies(
    groups: Option<&Vec<String>>,
    config: &Config,
    options: &InstallOptions,
) -> HuakResult<()> {
    let workspace = config.workspace();
    let package = workspace.current_local_metadata()?;
    let metadata = workspace.current_local_metadata()?;

    let binding = Vec::new(); // TODO
    let mut dependencies = Vec::new();

    if let Some(gs) = groups {
        // If the group "required" is passed and isn't a valid optional dependency group
        // then install just the required dependencies.
        if package
            .metadata()
            .optional_dependency_group("required")
            .is_none()
            && gs.contains(&"required".to_string())
        {
            if let Some(reqs) = package.metadata().dependencies() {
                dependencies.extend(reqs.iter().map(Dependency::from));
            }
        } else {
            gs.iter().for_each(|g| {
                package
                    .metadata()
                    .optional_dependency_group(g)
                    .unwrap_or(&binding)
                    .iter()
                    .for_each(|req| {
                        dependencies.push(Dependency::from(req));
                    });
            })
        }
    } else {
        // If no groups are passed then install all dependencies listed in the metadata file
        // including the optional dependencies.
        if let Some(reqs) = package.metadata().dependencies() {
            dependencies.extend(reqs.iter().map(Dependency::from));
        }
        if let Some(deps) = metadata.metadata().optional_dependencies() {
            deps.values().for_each(|reqs| {
                dependencies.extend(
                    reqs.iter().map(Dependency::from).collect::<Vec<_>>(),
                )
            });
        }
    }

    dependencies.dedup();

    if dependencies.is_empty() {
        return Ok(());
    }

    let python_env = workspace.resolve_python_environment()?;
    python_env.install_packages(&dependencies, options, config)
}

pub fn lint_project(config: &Config, options: &LintOptions) -> HuakResult<()> {
    let workspace = config.workspace();
    let package = workspace.current_package()?;
    let mut metadata = workspace.current_local_metadata()?;
    let python_env = workspace.resolve_python_environment()?;

    // Install `ruff` if it isn't already installed.
    let ruff_dep = Dependency::from_str("ruff")?;
    let mut lint_deps = vec![ruff_dep.clone()];
    if !python_env.contains_module("ruff")? {
        python_env.install_packages(
            &[&ruff_dep],
            &options.install_options,
            config,
        )?;
    }

    let mut terminal = config.terminal();

    if options.include_types {
        // Install `mypy` if it isn't already installed.
        let mypy_dep = Dependency::from_str("mypy")?;
        if !python_env.contains_module("mypy")? {
            python_env.install_packages(
                &[&mypy_dep],
                &options.install_options,
                config,
            )?;
        }

        // Keep track of the fact that `mypy` is a needed lint dep.
        lint_deps.push(mypy_dep);

        // Run `mypy` excluding the workspace's Python environment directory.
        let mut mypy_cmd = Command::new(python_env.python_path());
        make_venv_command(&mut mypy_cmd, &python_env)?;
        mypy_cmd
            .args(vec![
                "-m",
                "mypy",
                ".",
                "--exclude",
                python_env.name()?.as_str(),
            ])
            .current_dir(workspace.root());
        terminal.run_command(&mut mypy_cmd)?;
    }

    // Run `ruff`.
    let mut cmd = Command::new(python_env.python_path());
    let mut args = vec!["-m", "ruff", "check", "."];
    if let Some(v) = options.values.as_ref() {
        args.extend(v.iter().map(|item| item.as_str()));
    }
    make_venv_command(&mut cmd, &python_env)?;
    cmd.args(args).current_dir(workspace.root());
    terminal.run_command(&mut cmd)?;

    // Add installed lint deps (potentially both `mypy` and `ruff`) to metadata file if not already there.
    let new_lint_deps = lint_deps
        .iter()
        .filter(|dep| {
            !metadata
                .metadata()
                .contains_dependency_any(dep)
                .unwrap_or_default()
        })
        .map(|dep| dep.name())
        .collect::<Vec<_>>();

    if !new_lint_deps.is_empty() {
        for pkg in python_env
            .installed_packages()?
            .iter()
            .filter(|pkg| new_lint_deps.contains(&pkg.name()))
        {
            metadata.metadata_mut().add_optional_dependency(
                Dependency::from_str(&pkg.to_string())?,
                "dev",
            );
        }
    }

    if package.metadata() != metadata.metadata() {
        metadata.write_file()?;
    }

    Ok(())
}

pub fn list_python(config: &Config) -> HuakResult<()> {
    let env = Environment::new();

    // Print enumerated Python paths as they exist in the `PATH` environment variable.
    env.python_paths().enumerate().for_each(|(i, path)| {
        config
            .terminal()
            .print_custom(i + 1, path.display(), Color::Blue, false)
            .ok();
    });

    Ok(())
}

pub fn new_app_project(
    config: &Config,
    options: &WorkspaceOptions,
) -> HuakResult<()> {
    new_lib_project(config, options)?;

    let workspace = config.workspace();
    let mut metadata = workspace.current_local_metadata()?;

    let name = fs::last_path_component(workspace.root().as_path())?;
    let as_dep = Dependency::from_str(&name)?;
    metadata.metadata_mut().set_project_name(name);

    let src_path = workspace.root().join("src");
    let importable_name = importable_package_name(as_dep.name())?;
    std::fs::write(
        src_path.join(&importable_name).join("main.py"),
        DEFAULT_PYTHON_MAIN_FILE_CONTENTS,
    )?;
    let entry_point = default_entrypoint_string(&importable_name);
    metadata
        .metadata_mut()
        .add_script(as_dep.name(), &entry_point);

    metadata.write_file()
}

pub fn new_lib_project(
    config: &Config,
    options: &WorkspaceOptions,
) -> HuakResult<()> {
    let workspace = config.workspace();

    // Create a new metadata file or error if one exists.
    let mut metadata = match workspace.current_local_metadata() {
        Ok(_) => return Err(Error::ProjectFound),
        Err(_) => {
            LocalMetadata::template(workspace.root().join("pyproject.toml"))
        }
    };

    create_workspace(workspace.root())?;

    if options.uses_git {
        init_git(workspace.root())?;
    }

    let name = &fs::last_path_component(&config.workspace_root)?;
    metadata.metadata_mut().set_project_name(name.to_string());
    metadata.write_file()?;

    let as_dep = Dependency::from_str(name)?;
    let src_path = config.workspace_root.join("src");
    let importable_name = importable_package_name(as_dep.name())?;
    std::fs::create_dir_all(src_path.join(&importable_name))?;
    std::fs::create_dir_all(config.workspace_root.join("tests"))?;
    std::fs::write(
        src_path.join(&importable_name).join("__init__.py"),
        DEFAULT_PYTHON_INIT_FILE_CONTENTS,
    )?;
    std::fs::write(
        config.workspace_root.join("tests").join("test_version.py"),
        default_test_file_contents(&importable_name),
    )
    .map_err(Error::IOError)
}

pub fn publish_project(
    config: &Config,
    options: &PublishOptions,
) -> HuakResult<()> {
    let workspace = config.workspace();
    let package = workspace.current_package()?;
    let mut metadata = workspace.current_local_metadata()?;
    let python_env = workspace.resolve_python_environment()?;

    // Install `twine` if it isn't already installed.
    let pub_dep = Dependency::from_str("twine")?;
    if !python_env.contains_module(pub_dep.name())? {
        python_env.install_packages(
            &[&pub_dep],
            &options.install_options,
            config,
        )?;
    }

    // Add the installed `twine` package to the metadata file if it isn't already there.
    if !metadata.metadata().contains_dependency_any(&pub_dep)? {
        for pkg in python_env
            .installed_packages()?
            .iter()
            .filter(|pkg| pkg.name() == pub_dep.name())
        {
            metadata.metadata_mut().add_optional_dependency(
                Dependency::from_str(&pkg.to_string())?,
                "dev",
            );
        }
    }

    if package.metadata() != metadata.metadata() {
        metadata.write_file()?;
    }

    // Run `twine`.
    let mut cmd = Command::new(python_env.python_path());
    let mut args = vec!["-m", "twine", "upload", "dist/*"];
    if let Some(v) = options.values.as_ref() {
        args.extend(v.iter().map(|item| item.as_str()));
    }
    make_venv_command(&mut cmd, &python_env)?;
    cmd.args(args).current_dir(workspace.root());
    config.terminal().run_command(&mut cmd)
}

pub fn remove_project_dependencies(
    dependencies: &[String],
    config: &Config,
    options: &RemoveOptions,
) -> HuakResult<()> {
    let workspace = config.workspace();
    let package = workspace.current_package()?;
    let mut metadata = workspace.current_local_metadata()?;

    // Collect any dependencies to remove from the metadata file.
    let deps = dependency_iter(dependencies)
        .filter(|dep| {
            metadata
                .metadata()
                .contains_dependency_any(dep)
                .unwrap_or_default()
        })
        .collect::<Vec<_>>();

    if deps.is_empty() {
        return Ok(());
    }

    // Get all groups from the metadata file to include in the removal process.
    let mut groups = Vec::new();
    if let Some(deps) = metadata.metadata().optional_dependencies() {
        groups.extend(deps.keys().map(|key| key.to_string()));
    }
    for dep in &deps {
        metadata.metadata_mut().remove_dependency(dep);
        for group in &groups {
            metadata
                .metadata_mut()
                .remove_optional_dependency(dep, group);
        }
    }

    if package.metadata() != metadata.metadata() {
        metadata.write_file()?;
    }

    // Uninstall the dependencies from the Python environment if an environment is found.
    match workspace.current_python_environment() {
        Ok(it) => {
            it.uninstall_packages(&deps, &options.install_options, config)
        }
        Err(Error::PythonEnvironmentNotFound) => Ok(()),
        Err(e) => Err(e),
    }
}

pub fn run_command_str(command: &str, config: &Config) -> HuakResult<()> {
    let workspace = config.workspace();
    let python_env = workspace.current_python_environment()?;

    let mut cmd = Command::new(sys::shell_name()?);
    let flag = match OS {
        "windows" => "/C",
        _ => "-c",
    };
    make_venv_command(&mut cmd, &python_env)?;
    cmd.args([flag, command]).current_dir(&config.cwd);
    config.terminal().run_command(&mut cmd)
}

pub fn test_project(config: &Config, options: &TestOptions) -> HuakResult<()> {
    let workspace = config.workspace();
    let package = workspace.current_package()?;
    let mut metadata = workspace.current_local_metadata()?;
    let python_env = workspace.resolve_python_environment()?;

    // Install `pytest` if it isn't already installed.
    let test_dep = Dependency::from_str("pytest")?;
    if !python_env.contains_module(test_dep.name())? {
        python_env.install_packages(
            &[&test_dep],
            &options.install_options,
            config,
        )?;
    }

    // Add the installed `pytest` package to the metadata file if it isn't already there.
    if !metadata.metadata().contains_dependency_any(&test_dep)? {
        for pkg in python_env
            .installed_packages()?
            .iter()
            .filter(|pkg| pkg.name() == test_dep.name())
        {
            metadata.metadata_mut().add_optional_dependency(
                Dependency::from_str(&pkg.to_string())?,
                "dev",
            );
        }
    }

    if package.metadata() != metadata.metadata() {
        metadata.write_file()?;
    }

    // Run `pytest` with the package directory added to the command's `PYTHONPATH`.
    let mut cmd = Command::new(python_env.python_path());
    make_venv_command(&mut cmd, &python_env)?;
    let python_path = if workspace.root().join("src").exists() {
        workspace.root().join("src")
    } else {
        workspace.root().to_path_buf()
    };
    let mut args = vec!["-m", "pytest"];
    if let Some(v) = options.values.as_ref() {
        args.extend(v.iter().map(|item| item.as_str()));
    }
    cmd.args(args).env("PYTHONPATH", python_path);
    config.terminal().run_command(&mut cmd)
}

pub fn update_project_dependencies(
    dependencies: Option<Vec<String>>,
    config: &Config,
    options: &UpdateOptions,
) -> HuakResult<()> {
    let workspace = config.workspace();
    let package = workspace.current_package()?;
    let mut metadata = workspace.current_local_metadata()?;
    let python_env = workspace.resolve_python_environment()?;

    // Collect dependencies to update if they are listed in the metadata file.
    if let Some(it) = dependencies.as_ref() {
        let deps = dependency_iter(it)
            .filter_map(|dep| {
                if metadata
                    .metadata()
                    .contains_dependency_any(&dep)
                    .unwrap_or_default()
                {
                    Some(dep)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        if deps.is_empty() {
            return Ok(());
        }

        python_env.update_packages(&deps, &options.install_options, config)?;
    } else {
        let mut deps = metadata
            .metadata()
            .dependencies()
            .map(|reqs| reqs.iter().map(Dependency::from).collect::<Vec<_>>())
            .unwrap_or(Vec::new());

        if let Some(odeps) = metadata.metadata().optional_dependencies() {
            odeps.values().for_each(|reqs| {
                deps.extend(
                    reqs.iter().map(Dependency::from).collect::<Vec<_>>(),
                )
            });
        }

        deps.dedup();
        python_env.update_packages(&deps, &options.install_options, config)?;
    }

    // Get all groups from the metadata file to include in the removal process.
    let mut groups = Vec::new();
    if let Some(deps) = metadata.metadata().optional_dependencies() {
        groups.extend(deps.keys().map(|key| key.to_string()));
    }

    for pkg in python_env.installed_packages()? {
        let dep = &Dependency::from_str(&pkg.to_string())?;
        if metadata.metadata().contains_dependency(dep)? {
            metadata.metadata_mut().remove_dependency(dep);
            metadata.metadata_mut().add_dependency(dep.clone())
        }
        for g in groups.iter() {
            if metadata.metadata().contains_optional_dependency(dep, g)? {
                metadata.metadata_mut().remove_optional_dependency(dep, g);
                metadata
                    .metadata_mut()
                    .add_optional_dependency(dep.clone(), g);
            }
        }
    }

    if package.metadata() != metadata.metadata() {
        metadata.write_file()?;
    }

    Ok(())
}

pub fn use_python(version: &str, config: &Config) -> HuakResult<()> {
    let interpreters = Environment::resolve_python_interpreters();

    // Get a path to an interpreter based on the version provided.
    let path = match interpreters
        .interpreters()
        .iter()
        .find(|py| py.version().to_string() == version)
        .map(|py| py.path())
    {
        Some(it) => it,
        None => return Err(Error::PythonNotFound),
    };

    // Remove the current Python environment if one exists.
    let workspace = config.workspace();
    match workspace.current_python_environment() {
        Ok(it) => std::fs::remove_dir_all(it.root())?,
        Err(Error::PythonEnvironmentNotFound) => (),
        Err(e) => return Err(e),
    };

    // Create a new Python environment using the interpreter matching the version provided.
    let mut cmd = Command::new(path);
    cmd.args(["-m", "venv", ".venv"])
        .current_dir(&config.workspace_root);
    config.terminal().run_command(&mut cmd)
}

pub fn display_project_version(config: &Config) -> HuakResult<()> {
    let workspace = config.workspace();
    let package = workspace.current_package()?;

    let version = match package.metadata().project_version() {
        Some(it) => it,
        None => return Err(Error::PackageVersionNotFound),
    };

    config
        .terminal()
        .print_custom("version", version, Color::Green, false)
}

/// Make a `process::Command` a command with *virtual environment context*.
///
/// - Adds the virtual environment's executables directory path to the top of the command's
///   `PATH` environment variable.
/// - Adds `VIRTUAL_ENV` environment variable to the command pointing at the virtual environment's
///   root.
fn make_venv_command(
    cmd: &mut Command,
    venv: &PythonEnvironment,
) -> HuakResult<()> {
    let mut paths = env_path_values().unwrap_or(Vec::new());

    paths.insert(0, venv.executables_dir_path().clone());
    cmd.env(
        "PATH",
        std::env::join_paths(paths)
            .map_err(|e| Error::InternalError(e.to_string()))?,
    )
    .env("VIRTUAL_ENV", venv.root());

    Ok(())
}

/// Create a workspace directory on the system.
fn create_workspace<T: AsRef<Path>>(path: T) -> HuakResult<()> {
    let root = path.as_ref();

    if !root.exists() {
        std::fs::create_dir(root)?;
    } else {
        return Err(Error::DirectoryExists(root.to_path_buf()));
    }

    Ok(())
}

/// Initialize a directory for git.
///
/// - Initializes git
/// - Adds .gitignore if one doesn't already exist.
fn init_git<T: AsRef<Path>>(path: T) -> HuakResult<()> {
    let root = path.as_ref();

    if !root.join(".git").exists() {
        git::init(root)?;
    }
    let gitignore_path = root.join(".gitignore");
    if !gitignore_path.exists() {
        std::fs::write(gitignore_path, git::default_python_gitignore())?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        environment::env_path_string,
        fs,
        metadata::{default_pyproject_toml_contents, PyProjectToml},
        package::Package,
        sys::{TerminalOptions, Verbosity},
        test_resources_dir_path,
    };
    use tempfile::tempdir;

    #[test]

    fn test_add_project_dependencies() {
        let dir = tempdir().unwrap();
        fs::copy_dir(
            &test_resources_dir_path().join("mock-project"),
            &dir.path().join("mock-project"),
        )
        .unwrap();
        let root = dir.path().join("mock-project");
        let cwd = root.to_path_buf();
        let config = test_config(&root, &cwd, Verbosity::Quiet);
        let ws = config.workspace();
        let venv = ws.resolve_python_environment().unwrap();
        let options = AddOptions {
            install_options: InstallOptions { values: None },
        };

        add_project_dependencies(&[String::from("ruff")], &config, &options)
            .unwrap();

        let dep = Dependency::from_str("ruff").unwrap();
        let metadata = ws.current_local_metadata().unwrap();

        assert!(venv.contains_module("ruff").unwrap());
        assert!(metadata.metadata().contains_dependency(&dep).unwrap());
    }

    #[test]

    fn test_add_optional_project_dependencies() {
        let dir = tempdir().unwrap();
        fs::copy_dir(
            &test_resources_dir_path().join("mock-project"),
            &dir.path().join("mock-project"),
        )
        .unwrap();
        let group = "dev";
        let root = dir.path().join("mock-project");
        let cwd = root.to_path_buf();
        let config = test_config(&root, &cwd, Verbosity::Quiet);
        let ws = config.workspace();
        let venv = ws.resolve_python_environment().unwrap();
        let options = AddOptions {
            install_options: InstallOptions { values: None },
        };

        add_project_optional_dependencies(
            &[String::from("ruff")],
            group,
            &config,
            &options,
        )
        .unwrap();

        let dep = Dependency::from_str("ruff").unwrap();
        let metadata = ws.current_local_metadata().unwrap();

        assert!(venv.contains_module("ruff").unwrap());
        assert!(metadata
            .metadata()
            .contains_optional_dependency(&dep, "dev")
            .unwrap());
    }

    #[test]

    fn test_build_project() {
        let dir = tempdir().unwrap();
        fs::copy_dir(
            &test_resources_dir_path().join("mock-project"),
            &dir.path().join("mock-project"),
        )
        .unwrap();
        let root = dir.path().join("mock-project");
        let cwd = dir.path().to_path_buf();
        let config = test_config(root, cwd, Verbosity::Quiet);
        let options = BuildOptions {
            values: None,
            install_options: InstallOptions { values: None },
        };

        build_project(&config, &options).unwrap();
    }

    #[test]

    fn test_clean_project() {
        let dir = tempdir().unwrap();
        fs::copy_dir(
            test_resources_dir_path().join("mock-project"),
            dir.path().join("mock-project"),
        )
        .unwrap();
        let root = dir.path().join("mock-project");
        let cwd = root.to_path_buf();
        let config = test_config(root, cwd, Verbosity::Quiet);
        let options = CleanOptions {
            include_pycache: true,
            include_compiled_bytecode: true,
        };

        clean_project(&config, &options).unwrap();

        let dist = glob::glob(&format!(
            "{}",
            config.workspace_root.join("dist").join("*").display()
        ))
        .unwrap()
        .map(|item| item.unwrap())
        .collect::<Vec<_>>();
        let pycaches = glob::glob(&format!(
            "{}",
            config
                .workspace_root
                .join("**")
                .join("__pycache__")
                .display()
        ))
        .unwrap()
        .map(|item| item.unwrap())
        .collect::<Vec<_>>();
        let bytecode = glob::glob(&format!(
            "{}",
            config.workspace_root.join("**").join("*.pyc").display()
        ))
        .unwrap()
        .map(|item| item.unwrap())
        .collect::<Vec<_>>();

        assert!(dist.is_empty());
        assert!(pycaches.is_empty());
        assert!(bytecode.is_empty());
    }

    #[test]

    fn test_format_project() {
        let dir = tempdir().unwrap();
        fs::copy_dir(
            &test_resources_dir_path().join("mock-project"),
            &dir.path().join("mock-project"),
        )
        .unwrap();
        let root = dir.path().join("mock-project");
        let cwd = root.to_path_buf();
        let config = test_config(root, cwd, Verbosity::Quiet);
        let ws = config.workspace();
        let fmt_filepath =
            ws.root().join("src").join("mock_project").join("fmt_me.py");
        let pre_fmt_str = r#"
def fn( ):
    pass"#;
        std::fs::write(&fmt_filepath, pre_fmt_str).unwrap();
        let options = FormatOptions {
            values: None,
            install_options: InstallOptions { values: None },
        };

        format_project(&config, &options).unwrap();

        let post_fmt_str = std::fs::read_to_string(&fmt_filepath).unwrap();

        assert_eq!(
            post_fmt_str,
            r#"def fn():
    pass
"#
        );
    }

    #[test]

    fn test_init_lib_project() {
        let dir = tempdir().unwrap();
        std::fs::create_dir(dir.path().join("mock-project")).unwrap();
        let root = dir.path().join("mock-project");
        let cwd = root.to_path_buf();
        let config = test_config(root, cwd, Verbosity::Quiet);
        let options = WorkspaceOptions { uses_git: false };
        init_lib_project(&config, &options).unwrap();

        let ws = config.workspace();
        let metadata = ws.current_local_metadata().unwrap();

        assert_eq!(
            metadata.to_string_pretty().unwrap(),
            default_pyproject_toml_contents("mock-project")
        );
    }

    #[test]

    fn test_init_app_project() {
        let dir = tempdir().unwrap();
        std::fs::create_dir(dir.path().join("mock-project")).unwrap();
        let root = dir.path().join("mock-project");
        let cwd = root.to_path_buf();
        let config = test_config(root, cwd, Verbosity::Quiet);
        let options = WorkspaceOptions { uses_git: false };

        init_app_project(&config, &options).unwrap();

        let ws = config.workspace();
        let metadata = ws.current_local_metadata().unwrap();
        let pyproject_toml = PyProjectToml::default();
        pyproject_toml.project.clone().unwrap().name =
            String::from("mock-project");

        assert_eq!(
            metadata.to_string_pretty().unwrap(),
            r#"[build-system]
requires = ["hatchling"]
build-backend = "hatchling.build"

[project]
name = "mock-project"
version = "0.0.1"
description = ""
dependencies = []

[project.scripts]
mock-project = "mock_project.main:main"
"#
        );
    }

    #[test]

    fn test_install_project_dependencies() {
        let dir = tempdir().unwrap();
        fs::copy_dir(
            &test_resources_dir_path().join("mock-project"),
            &dir.path().join("mock-project"),
        )
        .unwrap();
        let root = dir.path().join("mock-project");
        let cwd = root.to_path_buf();
        let config = test_config(&root, &cwd, Verbosity::Quiet);
        let ws = config.workspace();
        let options = InstallOptions { values: None };
        let venv = ws.resolve_python_environment().unwrap();
        let test_package = Package::from_str("click==8.1.3").unwrap();
        let had_package = venv.contains_package(&test_package);

        install_project_dependencies(None, &config, &options).unwrap();

        assert!(!had_package);
        assert!(venv.contains_package(&test_package));
    }

    #[test]

    fn test_install_project_optional_dependencies() {
        let dir = tempdir().unwrap();
        fs::copy_dir(
            &test_resources_dir_path().join("mock-project"),
            &dir.path().join("mock-project"),
        )
        .unwrap();
        let root = dir.path().join("mock-project");
        let cwd = root.to_path_buf();
        let config = test_config(&root, &cwd, Verbosity::Quiet);
        let ws = config.workspace();
        let options = InstallOptions { values: None };
        let venv = ws.resolve_python_environment().unwrap();
        let had_package = venv.contains_module("pytest").unwrap();

        install_project_dependencies(
            Some(&vec![String::from("dev")]),
            &config,
            &options,
        )
        .unwrap();

        assert!(!had_package);
        assert!(venv.contains_module("pytest").unwrap());
    }

    #[test]

    fn test_lint_project() {
        let dir = tempdir().unwrap();
        fs::copy_dir(
            &test_resources_dir_path().join("mock-project"),
            &dir.path().join("mock-project"),
        )
        .unwrap();
        let root = dir.path().join("mock-project");
        let cwd = root.to_path_buf();
        let config = test_config(root, cwd, Verbosity::Quiet);
        let options = LintOptions {
            values: None,
            include_types: true,
            install_options: InstallOptions { values: None },
        };

        lint_project(&config, &options).unwrap();
    }

    #[test]

    fn test_fix_project() {
        let dir = tempdir().unwrap();
        fs::copy_dir(
            &test_resources_dir_path().join("mock-project"),
            &dir.path().join("mock-project"),
        )
        .unwrap();
        let root = dir.path().join("mock-project");
        let cwd = root.to_path_buf();
        let config = test_config(root, cwd, Verbosity::Quiet);
        let ws = config.workspace();
        let options = LintOptions {
            values: Some(vec![String::from("--fix")]),
            include_types: true,
            install_options: InstallOptions { values: None },
        };
        let lint_fix_filepath =
            ws.root().join("src").join("mock_project").join("fix_me.py");
        let pre_fix_str = r#"
import json # this gets removed(autofixed)


def fn():
    pass
"#;
        let expected = r#"


def fn():
    pass
"#;
        std::fs::write(&lint_fix_filepath, pre_fix_str).unwrap();

        lint_project(&config, &options).unwrap();

        let post_fix_str = std::fs::read_to_string(&lint_fix_filepath).unwrap();

        assert_eq!(post_fix_str, expected);
    }

    #[test]

    fn test_new_lib_project() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("mock-project");
        let cwd = root.to_path_buf();
        let config = test_config(root, cwd, Verbosity::Quiet);
        let options = WorkspaceOptions { uses_git: false };

        new_lib_project(&config, &options).unwrap();

        let ws = config.workspace();
        let metadata = ws.current_local_metadata().unwrap();
        let test_file_filepath =
            ws.root().join("tests").join("test_version.py");
        let test_file = std::fs::read_to_string(test_file_filepath).unwrap();
        let expected_test_file = r#"from mock_project import __version__


def test_version():
    __version__
"#;
        let init_file_filepath = ws
            .root()
            .join("src")
            .join("mock_project")
            .join("__init__.py");
        let init_file = std::fs::read_to_string(init_file_filepath).unwrap();
        let expected_init_file = "__version__ = \"0.0.1\"
";

        assert!(metadata.metadata().project().scripts.is_none());
        assert_eq!(test_file, expected_test_file);
        assert_eq!(init_file, expected_init_file);
    }

    #[test]

    fn test_new_app_project() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("mock-project");
        let cwd = root.to_path_buf();
        let config = test_config(root, cwd, Verbosity::Quiet);
        let options = WorkspaceOptions { uses_git: false };

        new_app_project(&config, &options).unwrap();

        let ws = config.workspace();
        let metadata = ws.current_local_metadata().unwrap();
        let main_file_filepath =
            ws.root().join("src").join("mock_project").join("main.py");
        let main_file = std::fs::read_to_string(main_file_filepath).unwrap();
        let expected_main_file = r#"def main():
    print("Hello, World!")


if __name__ == "__main__":
    main()
"#;

        assert_eq!(
            metadata.metadata().project().scripts.as_ref().unwrap()
                ["mock-project"],
            format!("{}.main:main", "mock_project")
        );
        assert_eq!(main_file, expected_main_file);
    }

    #[test]

    fn test_remove_project_dependencies() {
        let dir = tempdir().unwrap();
        fs::copy_dir(
            &test_resources_dir_path().join("mock-project"),
            &dir.path().join("mock-project"),
        )
        .unwrap();
        let root = dir.path().join("mock-project");
        let cwd = root.to_path_buf();
        let config = test_config(&root, &cwd, Verbosity::Quiet);
        let options = RemoveOptions {
            install_options: InstallOptions { values: None },
        };
        let ws = config.workspace();
        let venv = ws.resolve_python_environment().unwrap();
        let test_package = Package::from_str("click==8.1.3").unwrap();
        let test_dep = Dependency::from_str("click==8.1.3").unwrap();
        venv.install_packages(&[&test_dep], &options.install_options, &config)
            .unwrap();
        let metadata = ws.current_local_metadata().unwrap();
        let venv_had_package = venv.contains_package(&test_package);
        let toml_had_package =
            metadata.metadata().contains_dependency(&test_dep).unwrap();

        remove_project_dependencies(&["click".to_string()], &config, &options)
            .unwrap();

        let ws = config.workspace();
        let metadata = ws.current_local_metadata().unwrap();
        let venv_contains_package = venv.contains_package(&test_package);
        let toml_contains_package =
            metadata.metadata().contains_dependency(&test_dep).unwrap();

        assert!(venv_had_package);
        assert!(toml_had_package);
        assert!(!venv_contains_package);
        assert!(!toml_contains_package);
    }

    #[test]

    fn test_remove_project_optional_dependencies() {
        let dir = tempdir().unwrap();
        fs::copy_dir(
            &test_resources_dir_path().join("mock-project"),
            &dir.path().join("mock-project"),
        )
        .unwrap();
        let root = dir.path().join("mock-project");
        let cwd = root.to_path_buf();
        let config = test_config(&root, &cwd, Verbosity::Quiet);
        let options = RemoveOptions {
            install_options: InstallOptions { values: None },
        };
        let ws = config.workspace();
        let metadata = ws.current_local_metadata().unwrap();
        let venv = ws.resolve_python_environment().unwrap();
        let test_package = Package::from_str("black==22.8.0").unwrap();
        let test_dep = Dependency::from_str("black==22.8.0").unwrap();
        venv.install_packages(&[&test_dep], &options.install_options, &config)
            .unwrap();
        let venv_had_package =
            venv.contains_module(test_package.name()).unwrap();
        let toml_had_package = metadata
            .metadata()
            .contains_optional_dependency(&test_dep, "dev")
            .unwrap();

        remove_project_dependencies(&["black".to_string()], &config, &options)
            .unwrap();

        let ws = config.workspace();
        let metadata = ws.current_local_metadata().unwrap();
        let venv_contains_package = venv
            .contains_module(metadata.metadata().project_name())
            .unwrap();
        let toml_contains_package =
            metadata.metadata().contains_dependency(&test_dep).unwrap();

        assert!(venv_had_package);
        assert!(toml_had_package);
        assert!(!venv_contains_package);
        assert!(!toml_contains_package);
    }

    #[test]
    fn test_run_command_str() {
        let dir = tempdir().unwrap();
        fs::copy_dir(
            &test_resources_dir_path().join("mock-project"),
            &dir.path().join("mock-project"),
        )
        .unwrap();
        let root = dir.path().join("mock-project");
        let cwd = root.to_path_buf();
        let config = test_config(&root, &cwd, Verbosity::Quiet);
        let ws = config.workspace();
        // For some reason this test fails with multiple threads used. Workspace.resolve_python_environment()
        // ends up updating the PATH environment variable causing subsequent Python searches using PATH to fail.
        // TODO
        let env_path = env_path_string().unwrap();
        let venv = ws.resolve_python_environment().unwrap();
        std::env::set_var("PATH", env_path);
        let venv_had_package = venv.contains_module("black").unwrap();

        run_command_str("pip install black", &config).unwrap();

        let venv_contains_package = venv.contains_module("black").unwrap();

        assert!(!venv_had_package);
        assert!(venv_contains_package);
    }

    #[test]

    fn test_update_project_dependencies() {
        let dir = tempdir().unwrap();
        fs::copy_dir(
            &test_resources_dir_path().join("mock-project"),
            &dir.path().join("mock-project"),
        )
        .unwrap();
        let root = dir.path().join("mock-project");
        let cwd = root.to_path_buf();
        let config = test_config(root, cwd, Verbosity::Quiet);
        let options = UpdateOptions {
            install_options: InstallOptions { values: None },
        };

        update_project_dependencies(None, &config, &options).unwrap();
    }

    #[test]

    fn test_update_project_optional_dependencies() {
        let dir = tempdir().unwrap();
        fs::copy_dir(
            &test_resources_dir_path().join("mock-project"),
            &dir.path().join("mock-project"),
        )
        .unwrap();
        let root = dir.path().join("mock-project");
        let cwd = root.to_path_buf();
        let config = test_config(root, cwd, Verbosity::Quiet);
        let options = UpdateOptions {
            install_options: InstallOptions { values: None },
        };

        update_project_dependencies(None, &config, &options).unwrap();
    }

    #[cfg(unix)]
    #[test]

    fn test_use_python() {
        let dir = tempdir().unwrap();
        let interpreters = Environment::resolve_python_interpreters();
        let version = interpreters.latest().unwrap().version();
        let root = dir.path();
        let cwd = root;
        let config = test_config(root, cwd, Verbosity::Quiet);

        use_python(&version.to_string(), &config).unwrap();
    }

    #[test]

    fn test_test_project() {
        let dir = tempdir().unwrap();
        fs::copy_dir(
            &test_resources_dir_path().join("mock-project"),
            &dir.path().join("mock-project"),
        )
        .unwrap();
        let root = dir.path().join("mock-project");
        let cwd = root.to_path_buf();
        let config = test_config(root, cwd, Verbosity::Quiet);
        let options = TestOptions {
            values: None,
            install_options: InstallOptions { values: None },
        };

        test_project(&config, &options).unwrap();
    }

    fn test_config<T: AsRef<Path>>(
        root: T,
        cwd: T,
        verbosity: Verbosity,
    ) -> Config {
        let config = Config {
            workspace_root: root.as_ref().to_path_buf(),
            cwd: cwd.as_ref().to_path_buf(),
            terminal_options: TerminalOptions { verbosity },
        };

        config
    }
}
