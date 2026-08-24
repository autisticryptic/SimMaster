//! Qualcomm 410 (MSM8916-class) device driver.
//!
//! Everything specific to the Qualcomm 410 pocket-WiFi: exposing spare
//! `DATA*_CNTL` rpmsg channels as QMI endpoints, keeping them out of
//! ModemManager's hands (udev `ID_MM_PORT_IGNORE`), and running a retained WDS
//! session for user data so the IMS bearer never shares a slot with it.

pub mod baseband_faults;
pub mod ims_bearer;
pub mod secondary_qmi;
pub mod secondary_qmi_data;
