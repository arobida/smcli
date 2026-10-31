#[cfg(test)]
mod audit_ranking {
 #[test]
 fn audit_suggested_order() {
  let hit=|path:&str,picked:bool| smcli_core::service_protocol::FileHit {path:path.into(),insert:path.into(),provenance:if picked {vec!["files".into(),"picked".into()]}else{vec!["files".into()]},git:None,line:None,column:None,text:None,definition:None};
  let mut hits=vec![hit("needle.rs",false),hit("xneedle.rs",true)];
  super::suggested_first(&mut hits,"needle.rs");
  assert_eq!(hits[0].path,"xneedle.rs");
  println!("AUDIT: picker moved picked xneedle.rs ahead of exact needle.rs");
 }
}
