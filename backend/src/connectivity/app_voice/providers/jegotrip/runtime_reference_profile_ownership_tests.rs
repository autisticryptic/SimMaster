//! Actions-only accounting of direct Common profiles with exact raw reference storage.
use super::*;
#[test]
fn direct_common_profile_charges_exact_input_parsed_storage_and_legacy_canonical_string_without_dial(){
    let cert=Certificates::generate("IP:127.0.0.1");let raw="AccountEntry:sarc -p 00098 -h 42.7";let input=RuntimeCommonInputs{account_reference:raw,..common(&cert.root,12345)};let router_bytes=input.router.retained_bytes();let mut total=7;
    let captured=CommonProfile::capture(input,None,&mut total,false).unwrap();let entry=captured.entry.as_ref().unwrap();let canonical=captured.reference.as_ref().unwrap();
    assert_eq!(entry.original_factory_input(),raw.as_bytes());assert_eq!(canonical.as_str(),ENTRY);assert_eq!(total,7+entry.retained_bytes()+canonical.len()+router_bytes);assert_eq!(entry.identity(),b"AccountEntry");assert_eq!(captured.endpoint_index,0);assert!(captured.access.is_empty());assert!(!format!("{:?}",entry).contains("AccountEntry"));
}
#[test]
fn direct_common_profile_exact_reference_retention_does_not_expand_original_128k_bound(){
    let cert=Certificates::generate("IP:127.0.0.1");let raw="AccountEntry:sarc -p 00098 -h 42.7";let mut required=0;let probe=CommonProfile::capture(RuntimeCommonInputs{account_reference:raw,..common(&cert.root,12345)},None,&mut required,false).unwrap();drop(probe);
    let mut exact=MAX_PROFILE_BYTES-required;let captured=CommonProfile::capture(RuntimeCommonInputs{account_reference:raw,..common(&cert.root,12345)},None,&mut exact,false).unwrap();assert_eq!(exact,MAX_PROFILE_BYTES);assert_eq!(captured.entry.as_ref().unwrap().original_factory_input(),raw.as_bytes());drop(captured);
    let mut beyond=MAX_PROFILE_BYTES-required+1;assert_eq!(CommonProfile::capture(RuntimeCommonInputs{account_reference:raw,..common(&cert.root,12345)},None,&mut beyond,false).err(),Some(ProfileError::ResourceLimit));assert!(beyond>MAX_PROFILE_BYTES);
}
