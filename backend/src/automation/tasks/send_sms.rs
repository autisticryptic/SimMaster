use crate::automation::traits::AutomationTaskHandler;
use crate::db::beijing_sms_now_string;
use crate::modem_manager::send_sms;
use crate::state::AppState;
use anyhow::{Context, Result};
use ring::rand::{SecureRandom, SystemRandom};
use tracing::{info, warn};

pub struct SendSmsHandler;

fn generate_random_string(len: usize) -> String {
    let rng = SystemRandom::new();
    let mut bytes = vec![0u8; len];
    if rng.fill(&mut bytes).is_ok() {
        const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
        bytes
            .iter()
            .map(|&b| CHARS[(b as usize) % CHARS.len()] as char)
            .collect()
    } else {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let mut s = String::new();
        let mut val = now;
        for _ in 0..len {
            let idx = (val % 62) as usize;
            const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
            s.push(CHARS[idx] as char);
            val /= 62;
        }
        s
    }
}

fn get_random_u32(max: u32) -> u32 {
    if max == 0 {
        return 0;
    }
    let rng = SystemRandom::new();
    let mut bytes = [0u8; 4];
    if rng.fill(&mut bytes).is_ok() {
        u32::from_be_bytes(bytes) % max
    } else {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        (now % max as u128) as u32
    }
}

use futures_util::future::{BoxFuture, FutureExt};

impl AutomationTaskHandler for SendSmsHandler {
    fn task_type(&self) -> &'static str {
        "send_sms"
    }

    fn execute<'a>(
        &'a self,
        app: &'a AppState,
        params: &'a serde_json::Value,
    ) -> BoxFuture<'a, Result<()>> {
        let phone_number = match params
            .get("phone_number")
            .and_then(|v| v.as_str())
            .context("缺少接收号码")
        {
            Ok(pn) => pn.to_string(),
            Err(e) => return async move { Err(e) }.boxed(),
        };

        let content_template = match params
            .get("content")
            .and_then(|v| v.as_str())
            .context("缺少短信内容")
        {
            Ok(c) => c.to_string(),
            Err(e) => return async move { Err(e) }.boxed(),
        };

        let random_delay_seconds = params
            .get("random_delay_seconds")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;

        let retry_limit = params
            .get("retry_limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;

        async move {
            // 1. 随机延迟机制
            if random_delay_seconds > 0 {
                let delay = get_random_u32(random_delay_seconds);
                info!("Sms task delayed by {} seconds before sending", delay);
                tokio::time::sleep(tokio::time::Duration::from_secs(delay as u64)).await;
            }

            // 2. 渲染模板变量
            let time_str = beijing_sms_now_string();
            let rand_str = generate_random_string(6);
            let rendered_content = content_template
                .replace("{{时间}}", &time_str)
                .replace("{{随机字符串}}", &rand_str);

            // 3. 执行发送并实现重试
            let mut attempts = 0;
            loop {
                attempts += 1;
                match send_sms(&app.dbus_conn, &phone_number, &rendered_content).await {
                    Ok(_) => {
                        info!("Successfully sent automation SMS to {}", phone_number);
                        // 记录到数据库
                        let _ = app.database.insert_sms(
                            "outgoing",
                            &phone_number,
                            &rendered_content,
                            "sent",
                            None,
                        );
                        return Ok(());
                    }
                    Err(e) => {
                        warn!(
                            "Attempt {} to send automation SMS to {} failed: {:?}",
                            attempts, phone_number, e
                        );
                        if attempts > retry_limit {
                            // 记录失败的短信
                            let _ = app.database.insert_sms(
                                "outgoing",
                                &phone_number,
                                &rendered_content,
                                "failed",
                                None,
                            );
                            return Err(e).context("短信发送失败 (已达重试上限)");
                        }
                        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                    }
                }
            }
        }
        .boxed()
    }
}
