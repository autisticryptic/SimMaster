use super::*;
fn header(sequence:u16,peer:u16,echo:u16)->RouterHeader{RouterHeader{sequence,word1:peer,word2:echo,byte6:7,packet_kind:1,fragment_part:1}}

#[test]
fn no_received_observation_emits_native_unknown_echo_and_loss_not_invented_rtt(){
 let now=Instant::now();let mut path=PathTiming::new(now);let fields=path.timing(now+Duration::from_millis(123)).unwrap();
 assert_eq!((fields.word1,fields.word2,fields.byte6),(123,0,0));assert_eq!(path.metrics().unwrap().echoed_delay_ms,None);assert_eq!(path.probe_echo(),None);
}

#[test]
fn first_original_header_initializes_peer_clock_echo_delay_and_probe_echo(){
 let now=Instant::now();let mut path=PathTiming::new(now);let at=now+Duration::from_millis(100);
 assert!(path.observe(header(100,1000,80),at).unwrap().reset_receiver);let fields=path.timing(at).unwrap();
 assert_eq!((fields.word1,fields.word2,fields.byte6),(100,1000,0));assert_eq!(path.probe_echo(),Some((1000,100)));
 assert_eq!(path.metrics().unwrap(),PathMetrics{echoed_delay_ms:Some(20),receive_loss_percent:0,peer_loss:Some(7)});
}

#[test]
fn peer_clock_advances_999_per_complete_1000_local_ms_with_remainder(){
 let now=Instant::now();let mut path=PathTiming::new(now);path.observe(header(1,1000,0),now).unwrap();
 assert_eq!(path.timing(now+Duration::from_millis(2500)).unwrap().word2,3498);
 assert_eq!(path.timing(now+Duration::from_millis(3000)).unwrap().word2,3997);
}

#[test]
fn clock_echo_wraps_u16_and_maps_only_a_known_zero_echo_to_one(){
 let now=Instant::now();let mut path=PathTiming::new(now);path.observe(header(1,65530,0),now).unwrap();
 assert_eq!(path.timing(now+Duration::from_millis(10)).unwrap().word2,4);
 let mut zero=PathTiming::new(now);zero.observe(header(1,65526,0),now).unwrap();
 assert_eq!(zero.timing(now+Duration::from_millis(10)).unwrap().word2,1);
 let mut local=PathTiming::new(now);assert_eq!(local.timing(now+Duration::from_millis(65536)).unwrap().word1,0);
}

#[test]
fn independent_lag_and_absolute_deviation_vectors_match_recovered_conservative_echo(){
 let now=Instant::now();let mut path=PathTiming::new(now);path.observe(header(1,1000,0),now).unwrap();
 path.observe(header(2,1080,0),now+Duration::from_millis(100)).unwrap();
 assert_eq!(path.clock.as_ref().unwrap().lag.means().unwrap(),(10,10));assert_eq!(path.timing(now+Duration::from_millis(100)).unwrap().word2,1080);
 path.observe(header(3,1180,0),now+Duration::from_millis(200)).unwrap();
 assert_eq!(path.clock.as_ref().unwrap().lag.means().unwrap(),(13,10));assert_eq!(path.timing(now+Duration::from_millis(200)).unwrap().word2,1177);
 path.observe(header(4,1320,0),now+Duration::from_millis(300)).unwrap();
 assert_eq!(path.clock.as_ref().unwrap().lag.means().unwrap(),(10,10));assert_eq!(path.timing(now+Duration::from_millis(300)).unwrap().word2,1300);
}

#[test]
fn four_sample_buckets_replace_initial_zero_without_fabricated_idle_samples(){
 let now=Instant::now();let mut path=PathTiming::new(now);path.observe(header(1,1000,0),now).unwrap();
 for (index,peer) in [1979,2978,3977,4976].into_iter().enumerate(){path.observe(header(index as u16+2,peer,0),now+Duration::from_secs(index as u64+1)).unwrap();}
 assert_eq!(path.clock.as_ref().unwrap().lag.means().unwrap(),(20,10));assert_eq!(path.timing(now+Duration::from_secs(4)).unwrap().word2,4966);
 let mut idle=PathTiming::new(now);idle.observe(header(1,1000,0),now).unwrap();idle.observe(header(2,5985,0),now+Duration::from_secs(5)).unwrap();
 let lag=&idle.clock.as_ref().unwrap().lag;assert_eq!(lag.head,1);assert_eq!(lag.means().unwrap(),(5,5));
}

#[test]
fn out_of_range_peer_clock_resets_receive_window_not_local_monotonic_epoch(){
 let now=Instant::now();let mut path=PathTiming::new(now);path.observe(header(100,1000,0),now).unwrap();
 let at=now+Duration::from_millis(100);assert!(path.observe(header(101,9000,0),at).unwrap().reset_receiver);
 let fields=path.timing(at).unwrap();assert_eq!((fields.word1,fields.word2,fields.byte6),(100,9000,0));
 assert_eq!(path.probe_echo(),Some((9000,101)));
}

#[test]
fn negative_in_range_lag_rebases_peer_clock_without_erasing_observed_loss(){
 let now=Instant::now();let mut path=PathTiming::new(now);path.observe(header(100,1000,0),now).unwrap();
 assert!(!path.observe(header(103,1110,0),now+Duration::from_millis(100)).unwrap().reset_receiver);
 assert_eq!(path.metrics().unwrap().receive_loss_percent,50);assert_eq!(path.clock.as_ref().unwrap().word,1110);
}

#[test]
fn duplicate_sequence_does_not_refresh_echo_loss_clock_or_rtt_statistics(){
 let now=Instant::now();let mut path=PathTiming::new(now);path.observe(header(1,1000,20),now+Duration::from_millis(100)).unwrap();
 let mut duplicate=header(1,60000,50);duplicate.byte6=99;
 assert_eq!(path.observe(duplicate,now+Duration::from_millis(200)).unwrap(),TimingObservation{reset_receiver:false,feedback_due:false});
 assert_eq!(path.metrics().unwrap().echoed_delay_ms,Some(80));assert_eq!(path.metrics().unwrap().peer_loss,Some(7));
 assert_eq!(path.probe_echo(),Some((1000,1)));assert_eq!(path.timing(now+Duration::from_millis(200)).unwrap().word2,1100);
}

#[test]
fn native_sequence_window_reset_is_reported_even_with_a_smooth_peer_clock(){
 let now=Instant::now();let mut path=PathTiming::new(now);path.observe(header(1,1000,0),now).unwrap();
 assert!(path.observe(header(5000,1001,0),now+Duration::from_millis(1)).unwrap().reset_receiver);
 assert_eq!(path.metrics().unwrap().receive_loss_percent,0);assert_eq!(path.probe_echo(),Some((1001,5000)));
}

#[test]
fn echoed_delay_uses_signed_wrap_zero_floor_and_native_16000ms_ceiling(){
 let now=Instant::now();let mut path=PathTiming::new(now);
 path.observe(header(1,1000,65530),now+Duration::from_millis(65540)).unwrap();assert_eq!(path.metrics().unwrap().echoed_delay_ms,Some(10));
 path.observe(header(2,1010,30),now+Duration::from_millis(65550)).unwrap();assert_eq!(path.metrics().unwrap().echoed_delay_ms,Some(5));
 path.observe(header(3,1020,45560),now+Duration::from_millis(65560)).unwrap();assert_eq!(path.metrics().unwrap().echoed_delay_ms,Some(5336));
}

#[test]
fn zero_peer_echo_removes_delay_availability_without_claiming_zero_rtt(){
 let now=Instant::now();let mut path=PathTiming::new(now);path.observe(header(1,1000,10),now+Duration::from_millis(20)).unwrap();
 assert_eq!(path.metrics().unwrap().echoed_delay_ms,Some(10));path.observe(header(2,1010,0),now+Duration::from_millis(30)).unwrap();
 assert_eq!(path.metrics().unwrap().echoed_delay_ms,None);assert_eq!(path.metrics().unwrap().peer_loss,Some(7));
 path.observe(header(3,1020,20),now+Duration::from_millis(40)).unwrap();assert_eq!(path.metrics().unwrap().echoed_delay_ms,Some(20));
}

#[test]
fn gaps_raise_loss_and_late_unique_packets_fill_it_without_duplicate_delivery_policy(){
 let now=Instant::now();let mut path=PathTiming::new(now);path.observe(header(10,1000,0),now).unwrap();
 path.observe(header(13,1010,0),now+Duration::from_millis(10)).unwrap();assert_eq!(path.metrics().unwrap().receive_loss_percent,50);
 path.observe(header(11,1002,0),now+Duration::from_millis(20)).unwrap();assert_eq!(path.metrics().unwrap().receive_loss_percent,25);
 path.observe(header(12,1005,0),now+Duration::from_millis(30)).unwrap();assert_eq!(path.metrics().unwrap().receive_loss_percent,0);
}

#[test]
fn max_loss_across_8_16_32_64_sample_buckets_is_bounded_after_ring_wrap(){
 let now=Instant::now();let mut path=PathTiming::new(now);path.observe(header(0,1000,0),now).unwrap();
 for index in 1u16..=65{path.observe(header(index*2,1000u16.wrapping_add(index.wrapping_mul(999)),0),now+Duration::from_secs(index as u64)).unwrap();}
 assert_eq!(path.loss.as_ref().unwrap().head,1);assert_eq!(path.metrics().unwrap().receive_loss_percent,49);
}

#[test]
fn pre_initial_late_packets_cannot_wrap_negative_loss_into_byte_255(){
 let now=Instant::now();let mut path=PathTiming::new(now);path.observe(header(10,1000,0),now).unwrap();
 path.observe(header(9,1000,0),now+Duration::from_millis(1)).unwrap();assert_eq!(path.metrics().unwrap().receive_loss_percent,0);
 assert_eq!(path.timing(now+Duration::from_millis(1)).unwrap().byte6,0);
}

#[test]
fn hundred_unique_inputs_request_feedback_but_feedback_packets_never_loop(){
 let now=Instant::now();let mut path=PathTiming::new(now);
 for sequence in 1..100{assert!(!path.observe(header(sequence,1000,0),now).unwrap().feedback_due);}
 assert!(path.observe(header(100,1000,0),now).unwrap().feedback_due);
 let mut response=header(101,1000,0);response.packet_kind=3;
 assert!(!path.observe(response,now).unwrap().feedback_due);assert_eq!(path.unsignalled,0);
 path.observe(header(102,1000,0),now).unwrap();path.note_transmission();assert_eq!(path.unsignalled,0);
}

#[test]
fn clock_rollback_closes_statistics_without_migrating_them_to_a_new_epoch(){
 let now=Instant::now();let mut path=PathTiming::new(now);path.observe(header(1,1000,0),now+Duration::from_millis(10)).unwrap();
 assert_eq!(path.timing(now+Duration::from_millis(9)).err(),Some(TimingError::ClockRollback));
 assert_eq!(path.metrics().err(),Some(TimingError::Closed));assert!(path.probe_echo().is_none());
 assert_eq!(path.observe(header(2,1001,0),now+Duration::from_millis(20)).err(),Some(TimingError::Closed));
}

#[test]
fn statistics_counter_overflow_fails_closed_instead_of_wrapping_sample_counts(){
 let now=Instant::now();let mut path=PathTiming::new(now);path.observe(header(1,1000,0),now).unwrap();
 path.clock.as_mut().unwrap().lag.slots[0].count=u64::MAX;
 assert_eq!(path.observe(header(2,1001,0),now+Duration::from_millis(1)).err(),Some(TimingError::Overflow));
 assert_eq!(path.metrics().err(),Some(TimingError::Closed));
}

#[test]
fn timing_debug_does_not_expose_peer_clock_or_private_path_details(){
 let now=Instant::now();let mut path=PathTiming::new(now);path.observe(header(1,54321,0),now).unwrap();
 let text=format!("{path:?}");assert!(text.contains("<redacted>"));assert!(!text.contains("54321"));
}
