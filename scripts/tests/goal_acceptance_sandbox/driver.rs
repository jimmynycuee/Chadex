#![allow(dead_code)]
#[path = "../../../runtime-engine/src/tool_runtime/goal_acceptance_sandbox.rs"]
mod goal_acceptance_sandbox;
use goal_acceptance_sandbox::*;
use std::path::{Path, PathBuf};
use std::io::Write;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 4 { std::process::exit(64); }
    let mode = &args[0];
    let root = Path::new(&args[1]);
    let ports: Vec<u16> = if args[2] == "-" { vec![] } else {
        args[2].split(',').map(|s| s.parse().expect("port")).collect()
    };
    let separator = args.iter().position(|a| a == "--").expect("--");
    let readonly: Vec<PathBuf> = args[3..separator].iter().map(PathBuf::from).collect();
    let argv = &args[separator+1..];
    if mode.starts_with("docker-") {
        let fields: Vec<String> = std::env::vars()
            .filter(|(k,_)| k.starts_with("FIXTURE_DOCKER_")).map(|(k,v)| format!("{k}={v}")).collect();
        let value = |key: &str| fields.iter().find_map(|s|s.strip_prefix(&format!("FIXTURE_DOCKER_{key}="))).expect(key).to_owned();
        let backend = DockerBackend {
            executable: value("CLI").into(), endpoint: value("ENDPOINT"),
            image_id: value("IMAGE"), goal_run_id: value("GOAL"),
            commit_sha: value("COMMIT"), container_name: value("NAME"),
            uid: value("UID").parse().unwrap(), gid: value("GID").parse().unwrap(),
        };
        let result = match mode.as_str() {
            "docker-version" => docker_preflight(&backend),
            "docker-image" => docker_image_preflight(&backend),
            "docker-container" => docker_container_probe(&backend),
            "docker-inspect" => docker_inspect(&backend, &argv[0]),
            "docker-start" => docker_start(&backend, &DockerContainerIdentity {id:argv[0].clone(),created:argv[1].clone()}),
            "docker-wait" => docker_wait(&backend, &DockerContainerIdentity {id:argv[0].clone(),created:argv[1].clone()}),
            "docker-cleanup" => docker_cleanup(&backend,
                &DockerContainerIdentity {id:argv[0].clone(),created:argv[1].clone()},
                &DockerContainerFacts {id:argv[2].clone(),created:argv[3].clone(),name:argv[4].clone(),image_id:argv[5].clone(),goal_run_id:argv[6].clone(),commit_sha:argv[7].clone()}),
            "docker-create" => docker_create(root, argv, None, &backend, &readonly),
            "docker-run" => build_docker(root, argv, None, &backend, &readonly),
            _ => std::process::exit(64),
        };
        emit(result);
        return;
    }
    let result = match mode.as_str() {
        "offline" => build_with_read_only(root, argv, None, SandboxLane::Offline, &ports, &readonly),
        "preflight" => preflight_with_read_only(root, None, SandboxLane::Offline, &ports, &readonly),
        "loopback-production" => build_with_read_only(root, argv, None, SandboxLane::Loopback, &ports, &readonly),
        "loopback-owned-fixture" => fixture_build(root, argv, &ports, &readonly),
        _ => std::process::exit(64),
    };
    emit(result);
}

fn emit(result: Result<ProcessArgv, SandboxError>) {
    match result {
        Ok((executable,args,stdin)) => {
            let mut out = std::io::stdout().lock();
            for field in ["chadex-process-v2".to_owned(),if stdin.is_some(){"1"}else{"0"}.to_owned(),args.len().to_string()] {
                out.write_all(field.as_bytes()).unwrap();out.write_all(&[0]).unwrap();
            }
            for arg in std::iter::once(executable).chain(args).chain(std::iter::once(stdin.unwrap_or_default())) {
                out.write_all(arg.as_bytes()).unwrap();
                out.write_all(&[0]).unwrap();
            }
        }
        Err(error) => { eprintln!("{error}"); std::process::exit(78); }
    }
}
