//! Vendors' graphics engines (M13): Intel's display engine first ([`intel`], D268).

pub mod allwinner_de2;
pub mod amd_atom;
pub mod amd_atom_interpreter;
pub mod amd_dcn;
pub mod amd_discovery;
pub mod amd_dmcub;
pub mod intel;
pub mod intel_aux;
pub mod intel_combo_phy;
pub mod intel_cx0;
pub mod intel_dekel;
pub mod intel_dmc;
pub mod intel_hdmi;
pub mod intel_modeset;
pub mod intel_type_c;
pub mod nvidia_bios;
pub mod nvidia_display_mode;
pub mod nvidia_falcon;
pub mod nvidia_fwsec;
pub mod nvidia_gsp;
pub mod nvidia_gsp_boot;
pub mod nvidia_gsp_display;
pub mod nvidia_gsp_queue;
pub mod qualcomm_mdp5;
pub mod rockchip_vop;
