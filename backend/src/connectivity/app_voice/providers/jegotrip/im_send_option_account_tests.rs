//! Actions-only actual current SDK userSend with original notification DATA.
//! No original SO/device, notification service or paid business traffic.
use super::*;
#[tokio::test]
async fn send_account_actual_original_digest_userdata_online_imdn_are_not_body_auth_ack_and_all_joins(){
    Box::pin(async{
        for digest in ["display \"summary\" 文本",""]{
            let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let port=listener.local_addr().unwrap().port();let server=cert.server_config(&TLS13);let temp=TempDatabase::new();let manager=AccountManager::new(temp.database.clone());let account=manager.create(configuration(&cert.root,port,true),100).await.unwrap().account;let line=account.line_id.clone();let actual_digest=digest.to_owned();let (finish,finishing)=tokio::sync::oneshot::channel();
            let serving=tokio::spawn(Box::pin(async move{bounded(async move{
                let mut peer=CommonPeer::open(listener,server).await;login_with_distinct_main_account(&mut peer).await;
                let packet=query(&mut peer,n::im_send_data::USER_SEND_METHOD).await;assert_eq!(packet[0],0);let request=decode_request(&packet[1..]).unwrap();assert_eq!(request.method,b"userSend.Message.Message");let mut a=Reader::versioned(request.arguments).unwrap();assert_eq!(a.bytes().unwrap(),b"P2P/1_2");assert_eq!(a.bytes().unwrap(),b"Text");assert_eq!(a.bytes().unwrap(),br#"{"text":"actual body"}"#);assert!(a.properties().unwrap().is_empty());let params=a.properties().unwrap();assert_eq!(params.len(),4);assert_eq!(params.iter().find(|p|p.key==b"account").unwrap().value,OWN_URI);assert_ne!(params.iter().find(|p|p.key==b"account").unwrap().value,MAIN_ACCOUNT);
                let fields=a.properties().unwrap();assert_eq!(fields.len(),7);assert_eq!(fields.iter().find(|p|p.key==b"Notify.Text").unwrap().value,if actual_digest.is_empty(){b"actual body".as_slice()}else{actual_digest.as_bytes()});assert_eq!(fields.iter().find(|p|p.key==b"Notify.ImUserData").unwrap().value,b"");assert_eq!(fields.iter().find(|p|p.key==b"OnlineMsg").unwrap().value,b"01");assert_eq!(fields.iter().find(|p|p.key==b"Notify.ImdnId").unwrap().value,b"opaque-id");assert!(!fields.iter().any(|p|p.key==b"Notify.OnlineMsg"));assert_eq!(fields.iter().find(|p|p.key==b"Statistics.AppId").unwrap().value,b"fixture-app");a.finish().unwrap();peer.send(&[send_reply(request.call_id,true,0)]).await;finishing.await.unwrap();finish_account(&mut peer).await;
            }).await}));
            bounded(manager.start(&line,account.revision,100)).await.unwrap();phase(&manager,&line,"hosted").await;
            let message=||ImSendMessage::components(None,"actual body",&[],&[(b"userData".to_vec(),Vec::new()),(b"OnlineMsg".to_vec(),b"01".to_vec()),(b"imdnId".to_vec(),b"opaque-id".to_vec())],None).unwrap().with_notification_digest(digest).unwrap();
            assert_eq!(manager.send_im_message(&line,account.revision,"1_2",message(),NONCE,true,false).await.err(),Some("app_account_im_send_consent_required"));assert_eq!(rows(&temp.database,"app_voice_im_outbox",&line),0);
            let view=bounded(manager.send_im_message(&line,account.revision,"1_2",message(),NONCE,true,true)).await.unwrap();assert_eq!(view.state,"confirmed");let same=bounded(manager.send_im_message(&line,account.revision,"1_2",message(),NONCE,true,true)).await.unwrap();assert_eq!(same.id,view.id);assert_eq!(same.state,view.state);
            let changed=ImSendMessage::components(None,"actual body",&[],&[(b"userData".to_vec(),Vec::new()),(b"OnlineMsg".to_vec(),b"01".to_vec()),(b"imdnId".to_vec(),b"opaque-id".to_vec())],None).unwrap().with_notification_digest("changed-digest").unwrap();assert_eq!(bounded(manager.send_im_message(&line,account.revision,"1_2",changed,NONCE,true,true)).await.err(),Some("app_account_im_send_nonce_conflict"));
            no_other_business(&manager,&temp.database,&line);let public=serde_json::to_string(&manager.im_send_history(&line,128).unwrap()).unwrap();for private in ["actual body","display","opaque-id","OnlineMsg","ImUserData","fixture"]{assert!(!public.contains(private));}
            finish.send(()).unwrap();assert!(!bounded(manager.stop(&line,account.revision,100)).await.unwrap().io_cleanup_required);bounded(serving).await.unwrap();released(&temp.database,&line);
        }
    }).await;
}
