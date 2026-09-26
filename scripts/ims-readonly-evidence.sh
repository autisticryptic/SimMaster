#!/bin/sh
# Read-only, bounded IMS evidence. No AT/QMI, API mutation, service restart,
# bearer creation, configuration/DB reads, or raw SIP/identity output.
set -eu

filter_log() {
  perl -e '
    use strict;
    use warnings;
    my @events = (
      ["VoLTE IMS REGISTER request metadata prepared", "REGISTER_REQUEST"],
      ["VoLTE IMS REGISTER authentication challenge metadata received", "REGISTER_CHALLENGE"],
      ["VoLTE IMS authenticated REGISTER request metadata prepared", "REGISTER_AUTH_REQUEST"],
      ["VoLTE REGISTER transmit path", "REGISTER_TRANSPORT"],
      ["VoLTE IMS REGISTER failed before a complete response", "REGISTER_NO_COMPLETE_RESPONSE"],
      ["VoLTE IMS REGISTER terminal response metadata received", "REGISTER_TERMINAL_RESPONSE"],
      ["VoLTE IMS REGISTER success metadata received", "REGISTER_SUCCESS"],
      ["VoLTE IMS REGISTER refresh request metadata", "REGISTER_REFRESH_REQUEST"],
      ["VoLTE IMS restore attempt failed", "RESTORE_ATTEMPT_FAILED"],
      ["VoLTE IMS restore registered", "RESTORE_REGISTERED"],
      ["VoLTE IMS refresh scheduled", "REFRESH_SCHEDULED"],
      ["VoLTE IMS refresh rescheduled after successful REGISTER", "REFRESH_RESCHEDULED"],
      ["VoLTE REGISTER refresh will retry on the existing bearer", "REFRESH_RETRY"],
      ["VoLTE REGISTER refresh failed; rebuilding session", "REFRESH_REBUILD"],
      ["VoLTE REGISTER refresh failed; retaining protected bearer and session", "REFRESH_RETAINED"],
      ["VoLTE IMS registration lease expired before refresh attempt", "REGISTRATION_LEASE_EXPIRED"],
      ["VoLTE bearer settings contain no P-CSCF; querying retained-provider or exact-address AT observation", "PCSCF_OBSERVATION_STARTED"],
      ["VoLTE P-CSCF observation failed; explicit configuration and bearer DNS remain available", "PCSCF_OBSERVATION_FAILED"],
      ["VoLTE will try every discovered P-CSCF candidate", "PCSCF_CANDIDATES"],
      ["VoLTE P-CSCF candidate produced no SIP response; handing off to the next candidate", "PCSCF_NO_RESPONSE"],
      ["VoLTE retained IMS bearer ended", "BEARER_ENDED"],
      ["UE worker starting inside its namespace", "WORKER_START"],
      ["UE worker exiting", "WORKER_EXIT"]
    );
    my %boolean = map { $_ => 1 } qw(
      protected protected_channel authorization_present security_client_present security_verify_present
      route_header_present p_preferred_identity_present pani_present visited_network_present
      usable_security_server_present initial_security_client_present declared_require_sec_agree
      request_authorization_present request_security_client_present request_security_verify_present
      request_require_present request_proxy_require_present request_call_id_matches_initial
      request_route_present request_pani_present digest_challenge_present warning_present
      unsupported_present require_present proxy_require_present response_route_present
      contact_expiry_ambiguous wildcard_contact_present sec_agree_advertised sec_agree_required
      proxy_sec_agree_required mmtel_features_present icsi_ref_advertised audio_feature_advertised
      sip_instance_advertised sms_over_ip_advertised
    );
    my %number = (
      (map { $_ => 65535 } qw(local_port advertised_port actual_send_port send_route_port
        security_client_port_c security_client_port_s security_verify_port_c security_verify_port_s)),
      (map { $_ => 65535 } qw(auth_rounds security_server_count response_security_server_count
        service_route_count associated_uri_count contact_binding_count pcscf_count candidate_count
        accept_contact_count attempt refresh_count)),
      (map { $_ => 604800 } qw(expires_seconds lease_seconds refresh_after_seconds
        network_refresh_after_seconds test_override_seconds retry_after_seconds
        refresh_after_secs retry_after_secs lease_remaining_secs)),
      request_bytes => 1048576
    );
    my %enum = (
      register_phase => qr/\A(?:initial|refresh)\z/,
      local_family => qr/\Aipv[46]\z/,
      registration_mode => qr/\A(?:udp|ipsec|none)\z/,
      initial_authorization => qr/\A(?:aka_empty_uri_first|none)\z/
    );
    # Only known codes, never arbitrary error/detail strings. The API remains
    # necessary for codes absent from this small diagnostic allow-list.
    my %errors = map { $_ => 1 } qw(
      ims_register_initial_send_failed ims_register_initial_receive_failed
      ims_register_initial_unexpected_status ims_register_authenticated_send_failed
      ims_register_authenticated_receive_failed ims_register_authenticated_unexpected_status
      ims_register_auth_rejected ims_register_initial_min_expires_invalid
      ims_register_initial_min_expires_exhausted ims_register_initial_min_expires_unsupported
      ims_register_authenticated_min_expires_invalid ims_register_authenticated_min_expires_exhausted
      ims_register_authenticated_min_expires_unsupported
      cellular_ims_register_initial_unexpected_status cellular_ims_register_auth_unexpected_status
      cellular_ims_register_send_failed cellular_ims_register_auth_send_failed
      cellular_ims_usim_aka_failed cellular_ims_aka_material_invalid cellular_ims_aka_res_empty
      cellular_ims_digest_challenge_missing cellular_ims_digest_realm_missing
      cellular_ims_digest_nonce_missing cellular_ims_digest_nonce_decode_failed
      cellular_ims_bearer_session_lost cellular_ims_ip_settings_missing
      cellular_ims_runtime_all_pcscf_failed cellular_ims_pcscf_family_mismatch
      cellular_ims_runtime_mm_bearer_connect_failed cellular_ims_runtime_mm_bearer_not_connected
      cellular_ims_runtime_cellular_network_not_registered cellular_ims_runtime_ims_endpoint_unavailable
      cellular_ims_runtime_ims_bearer_start_failed cellular_ims_runtime_ue_worker_generation_changed
      cellular_ims_runtime_ims_baseband_wedged
    );
    my @output;
    my ($matched, $oversized, $failed) = (0, 0, 0);
    sub project {
      my ($line) = @_;
      if ($line eq "__JOURNAL_READ_FAILED__") { $failed = 1; return; }
      $line =~ s/\e\[[0-9;]*m//g;
      return if $line =~ /[\x00-\x08\x0b-\x1f\x7f]/;
      return unless $line =~ s/\A(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?(?:Z|[+-]\d{2}:\d{2}))\s+(?:TRACE|DEBUG|INFO|WARN|ERROR)\s+//;
      my $stamp = $1;
      # Current fmt has with_target(false); accept older Rust target prefixes.
      $line =~ s/\A[A-Za-z_][A-Za-z0-9_:]*:\s+//;
      my ($event, $tail);
      for my $entry (@events) {
        my ($message, $name) = @$entry;
        if ($line =~ /\A\Q$message\E(?=\s|\z)(.*)\z/) {
          ($event, $tail) = ($name, $1); last;
        }
      }
      return unless defined $event;
      my (%facts, %seen);
      # Consume complete quoted/Option values even for unknown keys. Display
      # errors are unquoted code:detail and can contain quotes or fake fields;
      # preserve only the code and treat the entire remainder as opaque.
      while (1) {
        if ($tail =~ /\G\s+error=(?!"|Some\()/gc) {
          my $remainder = substr($tail, pos($tail));
          my ($code) = $remainder =~ /\A([a-z][a-z0-9_]{0,127})(?=[:\s]|\z)/;
          if ($seen{error}++) { delete $facts{error}; }
          else { $facts{error} = defined($code) && $errors{$code} ? $code : "unlisted_error_redacted"; }
          last;
        }
        last unless $tail =~ /\G\s+([a-z_][a-z0-9_]*)=(Some\("(?:\\.|[^"\\])*"\)|Some\([^()\s"]+\)|"(?:\\.|[^"\\])*"|[^"\s]+)(?=\s|\z)/gc;
        my ($key, $value) = ($1, $2);
        if ($seen{$key}++) { delete $facts{$key}; next; }
        $value =~ s/\ASome\((.*)\)\z/$1/;
        $value =~ s/\A"(.*)"\z/$1/;
        if ($boolean{$key} && $value =~ /\A(?:true|false)\z/) {
          $facts{$key} = $value;
        } elsif (exists $number{$key} && $value =~ /\A\d{1,7}\z/ && $value <= $number{$key}) {
          $facts{$key} = 0 + $value;
        } elsif (exists $enum{$key} && $value =~ $enum{$key}) {
          $facts{$key} = $value;
        } elsif ($key eq "sip_status" && $value =~ /\A[1-6]\d{2}\z/) {
          $facts{$key} = $value;
        } elsif ($key =~ /\A(?:register|request|response)_cseq\z/ && $value =~ /\A(\d{1,9})(?: REGISTER)?\z/) {
          $facts{$key} = 0 + $1;
        } elsif ($key eq "error") {
          my ($code) = split /:/, $value, 2;
          $facts{error} = $errors{$code} ? $code : "unlisted_error_redacted";
        }
      }
      my $record = "$stamp $event";
      $record .= " $_=$facts{$_}" for sort keys %facts;
      push @output, $record;
      shift @output if @output > 120;
      $matched++;
    }
    # Bound memory even if a damaged log contains an enormous unterminated line.
    my ($pending, $discard) = ("", 0);
    while (1) {
      my $read = read(STDIN, my $chunk, 4096);
      if (!defined $read) { $failed = 1; last; }
      last if !$read;
      while (length $chunk) {
        my $end = index($chunk, "\n");
        my $part = $end < 0 ? $chunk : substr($chunk, 0, $end);
        $chunk = $end < 0 ? "" : substr($chunk, $end + 1);
        if (!$discard) {
          if (length($pending) + length($part) > 8192) {
            $pending = ""; $discard = 1; $oversized++;
          } else { $pending .= $part; }
        }
        if ($end >= 0) {
          $pending =~ s/\r\z//;
          project($pending) unless $discard;
          $pending = ""; $discard = 0;
        }
      }
    }
    project($pending) if length($pending) && !$discard;
    print "$_\n" for @output;
    print "LOG_FILTER_SUMMARY matched=$matched retained=", scalar(@output),
      " oversized=$oversized journal_read_failed=", $failed ? "true" : "false", "\n";
    exit($failed ? 1 : 0);
  '
}

case "${1-}" in
  --filter-log) [ "$#" -eq 1 ] || exit 2; filter_log; exit ;;
  --help) printf '%s\n' 'sh ims-readonly-evidence.sh [--filter-log]' 'Default: read local current-boot evidence. --filter-log: sanitize stdin only.'; exit ;;
  '') [ "$#" -eq 0 ] || exit 2 ;;
  *) printf '%s\n' 'Unsupported argument' >&2; exit 2 ;;
esac

command -v perl >/dev/null 2>&1 || { printf '%s\n' 'evidence_error=perl_unavailable'; exit 1; }
printf '%s\n' '__IMS_READ_ONLY_EVIDENCE_V2__'
printf 'observed_at_utc='; date -u '+%Y-%m-%dT%H:%M:%SZ'
printf 'uptime_seconds='; cut -d' ' -f1 /proc/uptime
printf 'boot_reference='; sha256sum /proc/sys/kernel/random/boot_id | cut -c1-16
# Installed metadata is NOT proof of the running executable revision.
if [ -r /opt/simadmin/meta.json ]; then
  head -c 16384 /opt/simadmin/meta.json | perl -0777 -ne '
    while (/"(version|commit|arch|build_time)"\s*:\s*"([^"\r\n]{1,80})"/g) {
      my ($k,$v)=($1,$2);
      my %allowed=(version=>qr/\A\d{1,3}\.\d{1,3}\.\d{1,3}(?:-[a-z]+\d{0,4})?\z/,
        commit=>qr/\A[0-9a-f]{7,40}\z/, arch=>qr/\A(?:arm64|amd64|aarch64|x86_64)(?:-unknown-linux-(?:musl|gnu))?\z/,
        build_time=>qr/\A\d{4}-\d{2}-\d{2}T[0-9:.Z+-]{8,25}\z/);
      print "installed_$k=$v\n" if $v =~ $allowed{$k};
    }
  '
fi
if [ -r /opt/simadmin/simadmin ]; then
  printf 'installed_program_sha256='; sha256sum /opt/simadmin/simadmin | cut -d' ' -f1
fi
process_reference() {
  pid=$(systemctl show simadmin.service -p MainPID --value 2>/dev/null || true)
  case "$pid" in ''|*[!0-9]*|0) printf 'unavailable\n'; return ;; esac
  if [ ! -r "/proc/$pid/stat" ]; then printf 'unavailable\n'; return; fi
  start=$(perl -ne 's/^\d+ \(.*\) //; my @f=split; print "$f[19]\n" if defined($f[19]) && $f[19]=~/\A\d+\z/' "/proc/$pid/stat" 2>/dev/null || true)
  case "$start" in ''|*[!0-9]*) printf 'unavailable\n' ;; *) printf '%s:%s\n' "$pid" "$start" ;; esac
}
before=$(process_reference)
printf 'running_process_reference=%s\n' "$before"
case "$before" in
  unavailable) ;;
  *) running_pid=${before%%:*}
     digest=$(sha256sum "/proc/$running_pid/exe" 2>/dev/null | cut -d' ' -f1 || true)
     printf '%s\n' "$digest" | grep -Eq '^[0-9a-f]{64}$' && printf 'running_program_sha256=%s\n' "$digest" || true ;;
esac
for unit in simadmin.service ModemManager.service simadmin-secondary-qmi.service; do
  printf 'service=%s\n' "$unit"
  systemctl show "$unit" -p MainPID -p NRestarts -p ActiveState -p SubState -p Type --no-pager 2>/dev/null |
    grep -E '^(MainPID|NRestarts)=[0-9]{1,10}$|^(ActiveState|SubState|Type)=[a-z-]{1,32}$' || true
done
printf '%s\n' 'journal_scope=current_boot_all_service_processes' '__REGISTER_METADATA_UNATTRIBUTED_UNTIL_LINE_CONFIRMED__'
{ journalctl -b -u simadmin.service --no-pager -o cat -n 6000 2>/dev/null || printf '%s\n' '__JOURNAL_READ_FAILED__'; } | filter_log
after=$(process_reference)
printf 'running_process_reference_after=%s\n' "$after"
if [ "$before" != unavailable ] && [ "$before" = "$after" ]; then
  printf '%s\n' 'running_process_stable_during_sample=true'
else
  printf '%s\n' 'running_process_stable_during_sample=false'
fi
