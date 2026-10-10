//! Read-only application/ISIM access for the process-based OpenSC adapter.
//! Each process reselects the captured application and EF. Only SELECT/READ
//! operations are replayed for GET RESPONSE/6C followups, never AUTHENTICATE.
use super::*;
use crate::connectivity::modems::ims::{
    cellular_ims::identity::UiccApplications,
    vowifi::qmi_uim::{isim, IsimImsMaterial},
};

pub fn read_ims_uicc(path: &str) -> Result<(Vec<Vec<u8>>, Option<IsimImsMaterial>), &'static str> {
    let reader = resolve_reader_blocking(path)?;
    let mut selection = Selection::default();
    let mut run = |apdus: &[Vec<u8>]| run_apdus_named(&reader.name, apdus);
    let aids = isim::read_application_aids_with(|apdu| selection.exchange(apdu, &mut run))?;
    let applications = UiccApplications::from_aids(aids.clone())?;
    let material = if let Some(aid) = applications.isim_aid {
        let response = selection.exchange(&select_application_apdu(&aid)?, &mut run)?;
        if (response.sw1, response.sw2) != (0x90, 0) { return Err("isim_select_failed"); }
        Some(isim::read_isim_material_with(|apdu| selection.exchange(apdu, &mut run))?)
    } else { None };
    Ok((aids, material))
}

#[derive(Default)]
struct Selection { root: Option<Vec<u8>>, file: Option<Vec<u8>> }

impl Selection {
    fn exchange(
        &mut self, apdu: &[u8],
        run: &mut impl FnMut(&[Vec<u8>]) -> Result<Vec<ApduResponse>, &'static str>,
    ) -> Result<UimApduResponse, &'static str> {
        if apdu.len() < 5 || apdu[0] != 0 || !matches!(apdu[1], 0xa4 | 0xb0 | 0xb2) {
            return Err("pcsc_ims_read_command_invalid");
        }
        let select = apdu[1] == 0xa4;
        let root = select && (apdu[2] == 4 || apdu.get(5..7) == Some(&[0x3f, 0][..]));
        let mut prefix = Vec::new();
        if !root {
            prefix.push(self.root.clone().ok_or("pcsc_ims_selection_missing")?);
            if !select {
                prefix.push(self.file.clone().ok_or("pcsc_ims_selection_missing")?);
            }
        }
        let mut commands = vec![apdu.to_vec()];
        for _ in 0..8 {
            let mut batch = prefix.clone();
            batch.extend(commands.iter().cloned());
            let responses = run(&batch)?;
            if responses.len() != batch.len() { return Err("pcsc_apdu_response_count_invalid"); }
            for response in &responses[..prefix.len()] {
                if !matches!((response.sw1, response.sw2), (0x90, 0) | (0x61, _) | (0x9f, _)) {
                    return Err("pcsc_ims_selection_changed");
                }
            }
            let last = responses.last().ok_or("pcsc_apdu_response_missing")?;
            if last.sw1 == 0x6c {
                *commands.last_mut().unwrap().last_mut().unwrap() = last.sw2;
                continue;
            }
            if matches!(last.sw1, 0x61 | 0x9f) {
                commands.push(build_get_response_apdu(last.sw2));
                continue;
            }
            let data: Vec<u8> = responses[prefix.len()..].iter().flat_map(|response| response.data.iter().copied()).collect();
            if data.len() > 8192 { return Err("isim_file_too_large"); }
            if select && (last.sw1, last.sw2) == (0x90, 0) {
                if root { self.root = Some(commands[0].clone()); self.file = None; }
                else { self.file = Some(commands[0].clone()); }
            }
            return Ok(UimApduResponse { data, sw1: last.sw1, sw2: last.sw2 });
        }
        Err("pcsc_ims_followup_limit")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ok() -> ApduResponse { ApduResponse { data: vec![], sw1: 0x90, sw2: 0 } }
    #[test]
    fn every_pcsc_read_reselects_application_and_file_without_replaying_authentication() {
        let mut selection = Selection::default();
        let app = select_application_apdu(isim::ISIM_AID_PREFIX).unwrap();
        let file = vec![0, 0xa4, 0, 4, 2, 0x6f, 2, 0];
        let read = vec![0, 0xb0, 0, 0, 8];
        let mut batches = Vec::new();
        let mut run = |commands: &[Vec<u8>]| { batches.push(commands.to_vec()); Ok(vec![ok(); commands.len()]) };
        selection.exchange(&app, &mut run).unwrap();
        selection.exchange(&file, &mut run).unwrap();
        selection.exchange(&read, &mut run).unwrap();
        assert_eq!(batches[2], vec![app, file, read]);
        assert!(selection.exchange(&[0, 0x88, 0, 0x81, 0], &mut |_| panic!("must not run")).is_err());
    }
    #[test]
    fn pcsc_corrected_select_is_retained_for_the_next_process() {
        let mut selection = Selection::default();
        let app = select_application_apdu(isim::ISIM_AID_PREFIX).unwrap();
        let mut calls = 0;
        selection.exchange(&app, &mut |commands| {
            calls += 1;
            if calls == 1 { Ok(vec![ApduResponse { data: vec![], sw1: 0x6c, sw2: 10 }]) }
            else { assert_eq!(commands[0].last(), Some(&10)); Ok(vec![ok()]) }
        }).unwrap();
        assert_eq!(selection.root.as_ref().unwrap().last(), Some(&10));
        selection.exchange(&[0, 0xa4, 0, 4, 2, 0x6f, 2, 0], &mut |commands| {
            assert_eq!(commands[0].last(), Some(&10));
            Ok(vec![ok(); commands.len()])
        }).unwrap();
    }

    #[test]
    fn pcsc_read_followups_are_bounded_and_do_not_hide_reselection_failure() {
        let mut selection = Selection { root: Some(vec![0, 0xa4, 0, 4, 2, 0x3f, 0, 0]), file: Some(vec![0, 0xa4, 0, 4, 2, 0x2f, 0, 0]) };
        let mut calls = 0;
        let result = selection.exchange(&[0, 0xb2, 1, 4, 8], &mut |commands| {
            calls += 1;
            let mut responses = vec![ok(); commands.len()];
            responses.last_mut().unwrap().sw1 = 0x61;
            Ok(responses)
        });
        assert_eq!(calls, 8);
        assert_eq!(result.unwrap_err(), "pcsc_ims_followup_limit");
        assert!(selection.exchange(&[0, 0xb2, 1, 4, 8], &mut |commands| {
            let mut responses = vec![ok(); commands.len()];
            responses[0].sw1 = 0x6a;
            Ok(responses)
        }).is_err());
    }
}
