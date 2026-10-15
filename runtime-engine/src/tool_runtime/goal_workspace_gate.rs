//! In-process mutation barrier for a Runner/root, including project aliases.
//! Read guards span admitted kernel mutations; Goal holds the write guard
//! across preparation, checkpoint and acceptance. Registered asynchronous jobs
//! are separately drained by the supervisor before it touches the workspace.
use std::collections::HashMap;
use std::sync::{Arc,Mutex,OnceLock,Weak};
use tokio::sync::RwLock;

pub(super) fn gate(runner:&str,root:&str)->Arc<RwLock<()>> {
    type Gates=Mutex<HashMap<(String,String),Weak<RwLock<()>>>>;
    static GATES:OnceLock<Gates>=OnceLock::new();
    let mut gates=GATES.get_or_init(Default::default).lock().expect("workspace gates");
    gates.retain(|_,gate|gate.strong_count()>0);
    let key=(runner.to_owned(),root.to_owned());
    if let Some(gate)=gates.get(&key).and_then(Weak::upgrade){return gate}
    let gate=Arc::new(RwLock::new(()));gates.insert(key,Arc::downgrade(&gate));gate
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn admitted_writes_drain_before_goal_and_other_projects_are_independent(){
        let first=gate("runner","/workspace");let alias=gate("runner","/workspace");
        let admitted=first.clone().try_read_owned().unwrap();
        assert!(alias.clone().try_write_owned().is_err());
        drop(admitted);
        let goal=first.clone().write_owned().await;
        assert!(alias.try_read_owned().is_err());
        assert!(gate("runner","/other").try_read_owned().is_ok());
        assert!(gate("different-runner","/workspace").try_read_owned().is_ok());
        drop(goal);assert!(first.try_read_owned().is_ok());
    }
}
