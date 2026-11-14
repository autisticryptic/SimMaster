//! Real loopback HTTPS, never a Provider/device request. Actions-only.
use super::*;
async fn live_query(operation:Value,path:&'static str,response:Value){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let port=listener.local_addr().unwrap().port();let server_config=cert.server_config(&rustls::version::TLS13);
    let responses=[("/login",json!({"code":0,"body":{"token":"fixture-live-communication-session"}})),("/user",json!({"code":0,"body":{"mobile":"7700900123","country_code":"44"}})),(path,json!({"code":0,"body":response}))];
    let worker=tokio::spawn(async move{
        tokio::time::timeout(Duration::from_secs(12),async move{
            for (expected,response_body) in responses{
                let (tcp,_)=listener.accept().await.unwrap();let mut stream=tokio_rustls::TlsAcceptor::from(server_config.clone()).accept(tcp).await.unwrap();
                let mut bytes=Vec::new();let mut buffer=[0u8;2048];
                let header_end=loop{
                    let n=stream.read(&mut buffer).await.unwrap();assert!(n>0);bytes.extend_from_slice(&buffer[..n]);assert!(bytes.len()<65536);
                    if let Some(end)=bytes.windows(4).position(|w|w==b"\r\n\r\n"){
                        let headers=String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();let length=headers.lines().find_map(|line|line.strip_prefix("content-length:")).map(|v|v.trim().parse::<usize>().unwrap()).unwrap_or(0);
                        if bytes.len()>=end+4+length{break end;}
                    }
                };
                let headers=String::from_utf8_lossy(&bytes[..header_end]);assert!(headers.starts_with(&format!("POST {expected}?")));assert!(!headers.contains("p@ss"));
                if expected!="/login"{assert!(headers.contains("token=fixture-live-communication-session"));}
                if expected=="/profile"{assert!(bytes[header_end+4..].is_empty());}
                if expected=="/phone-service"{let payload:Value=serde_json::from_slice(&bytes[header_end+4..]).unwrap();assert_eq!(payload,json!({"type":1}));}
                let body=serde_json::to_string(&response_body).unwrap();let response=format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());stream.write_all(response.as_bytes()).await.unwrap();let _=stream.shutdown().await;
            }
        }).await.unwrap();
    });
    let f=Fixture::new(Vec::new(),false);let manager=Arc::new(AppLoginManager::new(f.temp.database.clone(),Arc::downgrade(&f.accounts)));manager.initialize(f.temp.path.with_extension("missing-communication.json")).unwrap();
    let mut cfg=configuration(&f,false);cfg.http.base_url=Secret::from_owned(format!("https://127.0.0.1:{port}/"));cfg.http.root_der_base64=Some(Secret::from_owned(cert.root.clone()));manager.configure(ConfigurationWriteRequest{expected_revision:0,configuration:cfg}).unwrap();
    let task=id();manager.submit_jegotrip(request(&task,operation)).unwrap();let view=terminal(&manager,&task).await;assert_eq!(view.error,None);assert_eq!(view.communication.as_ref().unwrap().state,State::Observed);assert!(view.line_id.is_none());assert_eq!(f.factory.build_count(),0);assert_redacted(&view);worker.await.unwrap();manager.shutdown().await;
}
#[tokio::test]
async fn communication_real_https_profile_request_keeps_empty_body_pinned_identity_and_current_session_token(){
    live_query(json!({"kind":"query_profile"}),"/profile",profile_body()).await;
}
#[tokio::test]
async fn communication_real_https_phone_service_request_keeps_plain_numeric_query_shape(){
    live_query(json!({"kind":"query_phone_service"}),"/phone-service",json!({"phoneServerStatus":3})).await;
}
