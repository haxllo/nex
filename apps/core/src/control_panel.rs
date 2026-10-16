//! Control Panel applets and MMC consoles discovery.
//!
//! Scans `%WINDIR%\System32` and `%WINDIR%\SysWOW64` for `.cpl` and `.msc`
//! files with human descriptions. Icons extract from the files through
//! the regular pipeline and launching goes through ShellExecute, so no
//! shell-namespace COM is needed.

use crate::model::SearchItem;

/// Filename (lowercase) to display description.
fn describe(file_name: &str) -> Option<&'static str> {
    Some(match file_name {
        "access.cpl" => "Accessibility Options",
        "appwiz.cpl" => "Programs and Features",
        "bthprops.cpl" => "Bluetooth Devices",
        "colorcpl.cpl" => "Color Management",
        "desk.cpl" => "Display Settings",
        "firewall.cpl" => "Windows Defender Firewall",
        "hdwwiz.cpl" => "Add Hardware Wizard",
        "inetcpl.cpl" => "Internet Properties",
        "intl.cpl" => "Region and Language",
        "irprops.cpl" => "Infrared Device Settings",
        "joy.cpl" => "Game Controllers",
        "main.cpl" => "Mouse Properties",
        "mmsys.cpl" => "Sound",
        "ncpa.cpl" => "Network Connections",
        "powercfg.cpl" => "Power Options",
        "sticpl.cpl" => "Scanners and Cameras",
        "sysdm.cpl" => "System Properties",
        "tabletpc.cpl" => "Tablet PC Settings",
        "telephon.cpl" => "Phone and Modem",
        "timedate.cpl" => "Date and Time",
        "wscui.cpl" => "Security and Maintenance",
        "adsiedit.msc" => "Active Directory Service Interfaces Editor",
        "azman.msc" => "Authorization Manager",
        "certlm.msc" => "Certificates (Local Computer)",
        "certmgr.msc" => "Certificates (Current User)",
        "cluadmin.msc" => "Failover Cluster Manager",
        "compmgmt.msc" => "Computer Management",
        "comexp.msc" => "Component Services",
        "devmgmt.msc" => "Device Manager",
        "dfrgui.msc" => "Disk Defragmenter",
        "dhcpmgmt.msc" => "DHCP Server Management",
        "diskmgmt.msc" => "Disk Management",
        "dnsmgmt.msc" => "DNS Server Management",
        "domain.msc" => "Active Directory Domains and Trusts",
        "dsa.msc" => "Active Directory Users and Computers",
        "dssite.msc" => "Active Directory Sites and Services",
        "eventvwr.msc" => "Event Viewer",
        "fsmgmt.msc" => "Shared Folders",
        "gpmc.msc" => "Group Policy Management",
        "gpedit.msc" => "Group Policy Editor",
        "lusrmgr.msc" => "Local Users and Groups",
        "napclcfg.msc" => "Network Access Protection Client Configuration",
        "nps.msc" => "Network Policy Server",
        "perfmon.msc" => "Performance Monitor",
        "pkiview.msc" => "Enterprise PKI Health Monitor",
        "printmanagement.msc" => "Print Management",
        "rsop.msc" => "Resultant Set of Policy",
        "secpol.msc" => "Local Security Policy",
        "services.msc" => "Services",
        "storagemgmt.msc" => "Storage Management",
        "taskschd.msc" => "Task Scheduler",
        "tpm.msc" => "Trusted Platform Module Management",
        "tsconfig.msc" => "Remote Desktop Session Host Configuration",
        "virtmgmt.msc" => "Hyper-V Manager",
        "wbadmin.msc" => "Windows Server Backup",
        "wf.msc" => "Windows Defender Firewall with Advanced Security",
        "wmimgmt.msc" => "Windows Management Instrumentation",
        _ => return None,
    })
}

fn system_dirs() -> Vec<std::path::PathBuf> {
    let windir =
        std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".to_string());
    let root = std::path::PathBuf::from(windir);
    [root.join("System32"), root.join("SysWOW64")]
        .into_iter()
        .filter(|dir| dir.is_dir())
        .collect()
}

/// Enumerate Control Panel applets and MMC consoles. Best-effort:
/// unreadable directories simply contribute nothing.
pub fn control_panel_items() -> Vec<SearchItem> {
    let mut items = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for dir in system_dirs() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            let ext = path
                .extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            if ext != "cpl" && ext != "msc" {
                continue;
            }
            let file_name = match path
                .file_name()
                .and_then(|name| name.to_str())
            {
                Some(name) => name.to_ascii_lowercase(),
                None => continue,
            };
            if !seen.insert(file_name.clone()) {
                continue;
            }
            let Some(description) = describe(&file_name) else {
                continue;
            };
            let full = path.to_string_lossy().into_owned();
            items.push(SearchItem::from_owned_with_subtitle(
                format!("cpl:{full}"),
                "control-panel".to_string(),
                description.to_string(),
                full,
                description.to_string(),
                0,
                0,
            ));
        }
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpl_and_msc_applets_listed() {
        let items = control_panel_items();
        // Inbox on every Windows install.
        for want in ["Programs and Features", "Services", "Device Manager"] {
            assert!(
                items.iter().any(|item| item.title == want),
                "missing {want}"
            );
        }
    }

    #[test]
    fn applet_icon_extracts() {
        let items = control_panel_items();
        let appwiz = items
            .iter()
            .find(|item| item.title == "Programs and Features")
            .expect("appwiz.cpl");
        let cache = crate::overlay::icons::IconCache::new(8, 60_000);
        assert!(
            cache.png_bytes(&appwiz.path).is_some(),
            "appwiz icon extracts"
        );
        // .msc files carry no icon resource: the MMC host icon serves.
        let services = items
            .iter()
            .find(|item| item.title == "Services")
            .expect("services.msc");
        let png = cache.png_bytes(&services.path).expect("mmc icon extracts");
        let img = image::load_from_memory(&png).expect("mmc png decodes");
        assert_eq!(img.into_rgba8().dimensions(), (128, 128));
    }

    #[test]
    fn probe_msc_icon_stages() {
        let cache = crate::overlay::icons::IconCache::new(8, 60_000);
        for name in ["devmgmt.msc", "diskmgmt.msc", "services.msc", "appwiz.cpl", "mmc.exe"] {
            let path = format!("C:\\Windows\\System32\\{name}");
            let direct = cache.png_bytes(&path);
            match direct {
                Some(b) => match image::load_from_memory(&b) {
                    Ok(img) => {
                        let rgba = img.into_rgba8();
                        let (w, h) = rgba.dimensions();
                        let total = (w * h) as f64;
                        let content = rgba.pixels().filter(|p| p[3] > 8).count() as f64;
                        println!(
                            "msc stage probe [{name}]: dims={w}x{h} fill={:.1}% {} bytes",
                            content / total * 100.0,
                            b.len(),
                        );
                    }
                    Err(e) => println!("msc stage probe [{name}]: decode failed: {e}"),
                },
                None => println!("msc stage probe [{name}]: None"),
            }
        }
    }
}
