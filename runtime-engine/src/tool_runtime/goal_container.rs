// Included as a child of goal_supervisor, so only its trusted executor can use
// the Docker control plane. No model-facing tool accepts Docker capabilities.
use super::*;
use super::super::goal_acceptance_sandbox as sandbox;
use std::path::{Path, PathBuf};

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BackendPolicy {
    executable: PathBuf,
    endpoint: String,
    allowed_images: Vec<String>,
    uid: u32,
    gid: u32,
}

pub(super) fn backend(
    run: &GoalRunRecord, check: &crate::db::AcceptanceCheck, scope: &str,
) -> Result<sandbox::DockerBackend, ExecError> {
    let raw = std::env::var("CHADEX_GOAL_CONTAINER_POLICY").map_err(|_| ExecError::Denied(
        "LOOPBACK_BACKEND_UNAVAILABLE: configure the trusted Runner's local container policy; no host-network fallback".into()))?;
    let policy: BackendPolicy = serde_json::from_str(&raw).map_err(|_| ExecError::Denied("CONTAINER_POLICY_INVALID".into()))?;
    let image = check.container_image.as_ref().ok_or_else(|| ExecError::Denied(
        "LOOPBACK_BACKEND_UNAPPROVED: approve an acceptance revision with an immutable container_image".into()))?;
    if !policy.allowed_images.contains(image) { return Err(ExecError::Denied("CONTAINER_IMAGE_NOT_AUTHORIZED".into())); }
    let backend = sandbox::DockerBackend {
        executable: policy.executable, endpoint: policy.endpoint, image_id: image.clone(),
        goal_run_id: run.goal_run_id.clone(),
        commit_sha: run.checkpoint_sha.clone().unwrap_or_else(||run.binding.base_commit.clone()),
        container_name: format!("chadex-goal-{}",workspace_operation_id(&format!("{}:{scope}",run.goal_run_id))),
        uid: policy.uid, gid: policy.gid,
    };
    sandbox::docker_preflight(&backend).map_err(|e|ExecError::Denied(e.to_string()))?;
    Ok(backend)
}

fn spec(command: sandbox::ProcessArgv, timeout: u32) -> ProcessSpec {
    ProcessSpec{executable:command.0,args:command.1,stdin:command.2,cwd:None,timeout_secs:timeout}
}
fn denied(error: sandbox::SandboxError) -> ExecError { ExecError::Denied(error.to_string()) }

pub(super) async fn preflight(
    executor: &RuntimeGoalExecutor,run: &GoalRunRecord,workspace: &Workspace,
    check: &crate::db::AcceptanceCheck,
) -> Result<(),ExecError> {
    let b=backend(run,check,"capability-probe")?;
    let version=executor.environment_process(run,workspace,spec(sandbox::docker_preflight(&b).map_err(denied)?,15)).await?;
    let server:Value=serde_json::from_str(version.stdout_tail.trim()).unwrap_or(Value::Null);
    if !version.passed() || server["Os"]!="linux" {
        return Err(ExecError::Denied("CONTAINER_DAEMON_UNAVAILABLE: a verified local Linux daemon is required".into()));
    }
    let inspected=executor.environment_process(run,workspace,spec(sandbox::docker_image_preflight(&b).map_err(denied)?,15)).await?;
    let image:Value=serde_json::from_str(inspected.stdout_tail.trim()).unwrap_or(Value::Null);
    if !inspected.passed() || image["Id"]!=b.image_id || image["Os"]!="linux" || !image["Config"]["Volumes"].is_null() {
        return Err(ExecError::Denied("CONTAINER_IMAGE_UNAVAILABLE: exact local Linux image without declared volumes required; automatic pull is forbidden".into()));
    }
    Ok(())
}

fn facts(value: &Value) -> Result<sandbox::DockerContainerFacts,String> {
    let field=|key:&str|value[key].as_str().map(str::to_owned).ok_or_else(||format!("container inspect missing {key}"));
    let label=|key:&str|value["Config"]["Labels"][key].as_str().map(str::to_owned).ok_or_else(||"container ownership labels missing".to_owned());
    Ok(sandbox::DockerContainerFacts{id:field("Id")?,created:field("Created")?,name:field("Name")?,
        image_id:field("Image")?,goal_run_id:label("io.chadex.goal_run_id")?,commit_sha:label("io.chadex.commit_sha")?})
}
fn isolated(value:&Value,workspace:&Workspace,b:&sandbox::DockerBackend)->bool {
    let h=&value["HostConfig"];
    h["NetworkMode"]=="none" && h["ReadonlyRootfs"]==true && h["Privileged"]==false
        && h["CapDrop"]==json!(["ALL"]) && h["IpcMode"]=="private"
        && h["PidMode"].as_str()==Some("")
        && h["PortBindings"].as_object().is_none_or(|ports|ports.is_empty())
        && h["SecurityOpt"].as_array().is_some_and(|opts|opts.iter().any(|v|v.as_str().is_some_and(|s|s.starts_with("no-new-privileges"))))
        && value["Config"]["User"]==format!("{}:{}",b.uid,b.gid)
        && value["Mounts"].as_array().is_some_and(|mounts| {
            mounts.iter().any(|m|m["Destination"]=="/workspace" && m["Source"]==workspace.path && m["RW"]==false)
            && mounts.iter().all(|m| {
                (m["Type"]=="bind" && m["RW"]==false && m["Source"].as_str().is_some_and(|source|Path::new(source).starts_with(&workspace.path))
                    && m["Destination"].as_str().is_some_and(|target|Path::new(target).starts_with("/workspace")))
                || (m["Type"]=="tmpfs" && matches!(m["Destination"].as_str(),Some("/tmp"|"/workspace/.goal-acceptance")))
            })
        })
}

// Cleanup deliberately ignores a Goal control request: it can ONLY inspect and
// remove the exact ID/Created/image/labels captured before start. It cannot run
// project code. Stop/cancel must clean the container as well as its attach CLI.
async fn control_process(
    executor:&dyn GoalExecutor,run:&GoalRunRecord,workspace:&Workspace,spec:ProcessSpec,
)->Result<ProcessOutcome,ExecError>{
    let until=tokio::time::Instant::now()+Duration::from_secs(u64::from(spec.timeout_secs)+3);
    match executor.start(run,workspace,spec).await? {
        StartResult::Finished(outcome)=>Ok(outcome),
        StartResult::Started(job)=>loop {
            match executor.observe(run,workspace,&job).await? {
                ObserveResult::Finished(outcome)=>return Ok(outcome),
                ObserveResult::Lost(reason)=>return Err(ExecError::Unknown(reason)),
                ObserveResult::Running if tokio::time::Instant::now()<until=>tokio::time::sleep(Duration::from_millis(100)).await,
                ObserveResult::Running=>{let _=executor.stop(run,workspace,&job).await;return Err(ExecError::Unknown("container control timeout".into()))}
            }
        }
    }
}
async fn cleanup(
    executor:&dyn GoalExecutor,db:&Database,run:&GoalRunRecord,workspace:&Workspace,
    b:&sandbox::DockerBackend,identity:&sandbox::DockerContainerIdentity,
)->Result<(),String>{
    let inspected=control_process(executor,run,workspace,spec(sandbox::docker_inspect(b,&identity.id).map_err(|e|e.to_string())?,15)).await.map_err(|e|format!("{e:?}"))?;
    if !inspected.passed() {
        if inspected.stderr_tail.contains(&format!("No such container: {}",identity.id)) {
            db.goal_run_event(&run.goal_run_id,"container_cleaned",&json!({"id":identity.id,"created":identity.created,"already_absent":true}),now_unix_ms()).map_err(|e|e.to_string())?;
            return Ok(())
        }
        return Err("cleanup ownership inspect unavailable; preserved for operator recovery".into());
    }
    let value:Value=serde_json::from_str(inspected.stdout_tail.trim()).map_err(|_|"cleanup inspect truncated or invalid")?;
    let fresh=facts(&value)?;
    let command=sandbox::docker_cleanup(b,identity,&fresh).map_err(|e|e.to_string())?;
    let result=control_process(executor,run,workspace,spec(command,20)).await.map_err(|e|format!("{e:?}"))?;
    if !result.passed() {return Err("exact-ID cleanup failed; do not retry acceptance until reconciled".into())}
    db.goal_run_event(&run.goal_run_id,"container_cleaned",&json!({"id":identity.id,"created":identity.created}),now_unix_ms()).map_err(|e|e.to_string())?;
    Ok(())
}
struct CleanupGuard {
    executor:Arc<dyn GoalExecutor>, db:Arc<Database>,run:GoalRunRecord,workspace:Workspace,
    backend:sandbox::DockerBackend,identity:sandbox::DockerContainerIdentity,armed:bool,
}
impl Drop for CleanupGuard {
    fn drop(&mut self){
        if !self.armed{return}
        let (executor,db,run,workspace,b,id)=(self.executor.clone(),self.db.clone(),self.run.clone(),self.workspace.clone(),self.backend.clone(),self.identity.clone());
        tokio::spawn(async move {
            if let Err(error)=cleanup(executor.as_ref(),&db,&run,&workspace,&b,&id).await {
                let _=db.goal_run_event(&run.goal_run_id,"container_cleanup_pending",&json!({"id":id.id,"created":id.created,"reason":error}),now_unix_ms());
            }
        });
    }
}

impl GoalSupervisor {
    pub(super) async fn recover_owned_containers(&self){
        let entries=match self.db.goal_container_recovery_candidates(now_unix_ms()) {Ok(entries)=>entries,Err(error)=>{tracing::warn!(%error,"container recovery inventory unavailable");return}};
        for (run,value) in entries {
            let decode=||->Option<(Workspace,sandbox::DockerBackend,sandbox::DockerContainerIdentity)>{
                let b=&value["backend"];
                let text=|key:&str|b[key].as_str().map(str::to_owned);
                Some((Workspace{project_id:run.workspace_project_id.clone()?,path:run.workspace_path.clone()?},
                    sandbox::DockerBackend{executable:PathBuf::from(text("executable")?),endpoint:text("endpoint")?,image_id:text("image_id")?,
                        goal_run_id:text("goal_run_id")?,commit_sha:text("commit_sha")?,container_name:text("container_name")?,
                        uid:u32::try_from(b["uid"].as_u64()?).ok()?,gid:u32::try_from(b["gid"].as_u64()?).ok()?},
                    sandbox::DockerContainerIdentity{id:value["id"].as_str()?.into(),created:value["created"].as_str()?.into()}))
            };
            let Some((workspace,b,id))=decode() else{tracing::warn!(goal_run_id=%run.goal_run_id,"container ownership record cannot be decoded; preserved");continue};
            if b.goal_run_id!=run.goal_run_id {continue}
            if let Err(reason)=cleanup(self.executor.as_ref(),&self.db,&run,&workspace,&b,&id).await {
                tracing::warn!(goal_run_id=%run.goal_run_id,%reason,"owned container cleanup remains blocked");
            }
        }
    }

    pub(super) async fn run_container_check(
        &self,run:&GoalRunRecord,workspace:&Workspace,lease:&GoalLease,lost:&AtomicBool,
        index:usize,check:&crate::db::AcceptanceCheck,probe:bool,
    )->Result<OpOutcome,Exit>{
        let phase=if probe{format!("probe:g{}",lease.generation)}else{"verify".into()};
        let scope=format!("{}:r{}:a{}:{index}:{phase}",run.checkpoint_sha.as_deref().unwrap_or(&run.binding.base_commit),run.acceptance_revision,run.current_attempt);
        let b=match backend(run,check,&scope){Ok(b)=>b,Err(error)=>return Ok(OpOutcome::Park(Park::new(GoalRunState::NeedsAttention,format!("{error:?}"))))};
        let key=format!("container:{scope}");
        let probe_target=if std::path::Path::new(&check.argv[0]).file_name().and_then(|s|s.to_str())==Some("env") {
            check.argv.iter().skip(1).find(|arg|!arg.contains('=')).cloned().unwrap_or_default()
        }else{check.argv[0].clone()};
        let argv=if probe {vec!["/bin/sh".into(),"-c".into(),
            "case \"$1\" in /*) test -x \"$1\";; *) command -v -- \"$1\" >/dev/null;; esac".into(),"goal-toolchain-probe".into(),probe_target]}
            else{check.argv.clone()};
        let command=match sandbox::docker_create(Path::new(&workspace.path),&argv,check.cwd.as_deref().map(Path::new),&b,&[]){Ok(c)=>c,Err(e)=>return Ok(OpOutcome::Park(Park::new(GoalRunState::NeedsAttention,e.to_string())))};
        let created=self.run_op(run,workspace,lease,lost,&format!("{key}:create"),"container_create",run.committed_rounds,spec(command,20),OpPolicy::ExactlyOnce).await?;
        let id=match created {
            OpOutcome::Done(ref out) if out.passed()=>sandbox::docker_created_id(&out.stdout_tail).ok(),
            _=>None,
        };
        let Some(id)=id else {
            // No payload was started. A create with unknown outcome is NEVER
            // repeated; the stable UUID name is for recovery inspection only.
            return Ok(OpOutcome::Park(Park::new(GoalRunState::NeedsAttention,"container_creation_unresolved: inspect the journal's owned UUID before retry; no acceptance command started")));
        };
        let inspected=control_process(self.executor.as_ref(),run,workspace,spec(sandbox::docker_inspect(&b,&id).map_err(|e|Exit::Store(PlanStoreError::new("goal_container_invalid",e.to_string())))?,15)).await;
        let value=match inspected{Ok(out)if out.passed()=>serde_json::from_str::<Value>(out.stdout_tail.trim()).ok(),_=>None};
        let Some(value)=value else{return Ok(OpOutcome::Park(Park::new(GoalRunState::NeedsAttention,"container_inspection_unresolved: created ID preserved; no payload started")))};
        let fresh=match facts(&value){Ok(f)=>f,Err(e)=>return Ok(OpOutcome::Park(Park::new(GoalRunState::NeedsAttention,e)))};
        let identity=sandbox::DockerContainerIdentity{id,created:fresh.created.clone()};
        if sandbox::verify_docker_ownership(&b,&identity,&fresh).is_err() || !isolated(&value,workspace,&b) {
            return Ok(OpOutcome::Park(Park::new(GoalRunState::NeedsAttention,"container_isolation_unverified: no payload started")));
        }
        self.event(&run.goal_run_id,"container_owned",json!({"id":identity.id,"created":identity.created,"backend":{
            "executable":b.executable,"endpoint":b.endpoint,"image_id":b.image_id,"container_name":b.container_name,
            "goal_run_id":b.goal_run_id,"commit_sha":b.commit_sha,"uid":b.uid,"gid":b.gid},"key":key}));
        let mut guard=CleanupGuard{executor:self.executor.clone(),db:self.db.clone(),run:run.clone(),workspace:workspace.clone(),backend:b.clone(),identity:identity.clone(),armed:true};
        let start=sandbox::docker_start(&b,&identity).map_err(|e|Exit::Store(PlanStoreError::new("goal_container_invalid",e.to_string())))?;
        let attached=self.run_op(run,workspace,lease,lost,&format!("{key}:start"),"container_start",run.committed_rounds,spec(start,if probe{15}else{check.timeout_secs}),OpPolicy::ExactlyOnce).await?;
        let outcome=match attached {
            OpOutcome::Park(park)=>OpOutcome::Park(park),
            OpOutcome::Done(mut output)=>{
                let waited=self.run_op(run,workspace,lease,lost,&format!("{key}:wait"),"container_wait",run.committed_rounds,spec(sandbox::docker_wait(&b,&identity).map_err(|e|Exit::Store(PlanStoreError::new("goal_container_invalid",e.to_string())))?,10),OpPolicy::Reobservable).await?;
                match waited {
                    OpOutcome::Park(park)=>OpOutcome::Park(park),
                    OpOutcome::Done(wait)=>{
                        output.exit_code=if wait.passed(){sandbox::docker_wait_exit_code(&wait.stdout_tail).ok().map(i64::from)}else{None};
                        // CLI attach success is NEVER a substitute for actual daemon exit.
                        OpOutcome::Done(output)
                    }
                }
            }
        };
        match cleanup(self.executor.as_ref(),&self.db,run,workspace,&b,&identity).await {
            Ok(())=>{guard.armed=false;Ok(outcome)},
            Err(reason)=>{self.event(&run.goal_run_id,"container_cleanup_pending",json!({"id":identity.id,"created":identity.created,"reason":reason}));Ok(OpOutcome::Park(Park::new(GoalRunState::NeedsAttention,"container_cleanup_pending: reconcile the exact owned container before retry")))}
        }
    }
}
