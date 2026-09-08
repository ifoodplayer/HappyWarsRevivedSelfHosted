//! Forwarding stubs for all d3d11.dll exports.
//!
//! D3D11CreateDevice and D3D11CreateDeviceAndSwapChain intercept the first
//! call to apply memory patches before delegating to the real implementation.
//! All other exports are forwarded directly.

use windows::{
    core::HRESULT,
    Win32::{
        Foundation::{E_FAIL, HMODULE},
        Graphics::{
            Direct3D::{D3D_DRIVER_TYPE, D3D_FEATURE_LEVEL},
            Direct3D11::{ID3D11Device, ID3D11DeviceContext},
            Dxgi::{IDXGIAdapter, IDXGISwapChain, DXGI_SWAP_CHAIN_DESC},
        },
    },
};

use crate::{ensure_patched, get_proc_raw};

/// Generates a forwarding export that resolves and caches the proc address by name.
macro_rules! fwd {
    ($name:ident) => {
        #[no_mangle]
        pub unsafe extern "system" fn $name() {
            static FN: std::sync::OnceLock<unsafe extern "system" fn()> =
                std::sync::OnceLock::new();
            let f = FN.get_or_init(|| {
                let sym = concat!(stringify!($name), "\0");
                crate::get_proc_raw(sym.as_bytes())
                    .expect(concat!("d3d11: cannot find ", stringify!($name)))
            });
            f()
        }
    };
}

fwd!(D3D11CoreCreateDevice);
fwd!(D3D11CoreCreateLayeredDevice);
fwd!(D3D11CoreRegisterLayers);
fwd!(D3D11On12CreateDevice);
fwd!(EnableFeatureLevelUpgrade);

#[no_mangle]
pub unsafe extern "system" fn D3D11CoreGetLayeredDeviceSize() -> usize {
    type Fn = unsafe extern "system" fn() -> usize;
    static FN: std::sync::OnceLock<Fn> = std::sync::OnceLock::new();
    let f = FN.get_or_init(|| {
        std::mem::transmute(
            get_proc_raw(b"D3D11CoreGetLayeredDeviceSize\0")
                .expect("d3d11: D3D11CoreGetLayeredDeviceSize not found"),
        )
    });
    f()
}

#[no_mangle]
pub unsafe extern "system" fn D3D11CreateDevice(
    pAdapter:           *mut IDXGIAdapter,
    DriverType:         D3D_DRIVER_TYPE,
    Software:           HMODULE,
    Flags:              u32,
    pFeatureLevels:     *const D3D_FEATURE_LEVEL,
    FeatureLevels:      u32,
    SDKVersion:         u32,
    ppDevice:           *mut *mut ID3D11Device,
    pFeatureLevel:      *mut D3D_FEATURE_LEVEL,
    ppImmediateContext: *mut *mut ID3D11DeviceContext,
) -> HRESULT {
    if !ppDevice.is_null()           { *ppDevice = std::ptr::null_mut(); }
    if !pFeatureLevel.is_null()      { *pFeatureLevel = D3D_FEATURE_LEVEL(0); }
    if !ppImmediateContext.is_null() { *ppImmediateContext = std::ptr::null_mut(); }

    ensure_patched();

    type Fn = unsafe extern "system" fn(
        *mut IDXGIAdapter, D3D_DRIVER_TYPE, HMODULE, u32,
        *const D3D_FEATURE_LEVEL, u32, u32,
        *mut *mut ID3D11Device, *mut D3D_FEATURE_LEVEL,
        *mut *mut ID3D11DeviceContext,
    ) -> HRESULT;

    let raw = match get_proc_raw(b"D3D11CreateDevice\0") {
        Some(p) => p,
        None => return E_FAIL,
    };
    let func: Fn = std::mem::transmute(raw);

    let mut out_dev: *mut ID3D11Device       = std::ptr::null_mut();
    let mut out_ctx: *mut ID3D11DeviceContext = std::ptr::null_mut();
    let mut out_lvl: D3D_FEATURE_LEVEL       = D3D_FEATURE_LEVEL(0);

    let hr = func(
        pAdapter, DriverType, Software, Flags,
        pFeatureLevels, FeatureLevels, SDKVersion,
        &mut out_dev, &mut out_lvl, &mut out_ctx,
    );

    if hr.is_ok() {
        if !ppDevice.is_null()           { *ppDevice = out_dev; }
        if !pFeatureLevel.is_null()      { *pFeatureLevel = out_lvl; }
        if !ppImmediateContext.is_null() { *ppImmediateContext = out_ctx; }
    }
    hr
}

#[no_mangle]
pub unsafe extern "system" fn D3D11CreateDeviceAndSwapChain(
    pAdapter:           *mut IDXGIAdapter,
    DriverType:         D3D_DRIVER_TYPE,
    Software:           HMODULE,
    Flags:              u32,
    pFeatureLevels:     *const D3D_FEATURE_LEVEL,
    FeatureLevels:      u32,
    SDKVersion:         u32,
    pSwapChainDesc:     *const DXGI_SWAP_CHAIN_DESC,
    ppSwapChain:        *mut *mut IDXGISwapChain,
    ppDevice:           *mut *mut ID3D11Device,
    pFeatureLevel:      *mut D3D_FEATURE_LEVEL,
    ppImmediateContext: *mut *mut ID3D11DeviceContext,
) -> HRESULT {
    if !ppSwapChain.is_null()        { *ppSwapChain = std::ptr::null_mut(); }
    if !ppDevice.is_null()           { *ppDevice = std::ptr::null_mut(); }
    if !pFeatureLevel.is_null()      { *pFeatureLevel = D3D_FEATURE_LEVEL(0); }
    if !ppImmediateContext.is_null() { *ppImmediateContext = std::ptr::null_mut(); }

    ensure_patched();

    type Fn = unsafe extern "system" fn(
        *mut IDXGIAdapter, D3D_DRIVER_TYPE, HMODULE, u32,
        *const D3D_FEATURE_LEVEL, u32, u32,
        *const DXGI_SWAP_CHAIN_DESC,
        *mut *mut IDXGISwapChain, *mut *mut ID3D11Device,
        *mut D3D_FEATURE_LEVEL, *mut *mut ID3D11DeviceContext,
    ) -> HRESULT;

    let raw = match get_proc_raw(b"D3D11CreateDeviceAndSwapChain\0") {
        Some(p) => p,
        None => return E_FAIL,
    };
    let func: Fn = std::mem::transmute(raw);

    let mut out_sc:  *mut IDXGISwapChain     = std::ptr::null_mut();
    let mut out_dev: *mut ID3D11Device       = std::ptr::null_mut();
    let mut out_ctx: *mut ID3D11DeviceContext = std::ptr::null_mut();
    let mut out_lvl: D3D_FEATURE_LEVEL       = D3D_FEATURE_LEVEL(0);

    let hr = func(
        pAdapter, DriverType, Software, Flags,
        pFeatureLevels, FeatureLevels, SDKVersion,
        pSwapChainDesc,
        &mut out_sc, &mut out_dev, &mut out_lvl, &mut out_ctx,
    );

    if hr.is_ok() {
        if !ppSwapChain.is_null()        { *ppSwapChain = out_sc; }
        if !ppDevice.is_null()           { *ppDevice = out_dev; }
        if !pFeatureLevel.is_null()      { *pFeatureLevel = out_lvl; }
        if !ppImmediateContext.is_null() { *ppImmediateContext = out_ctx; }
    }
    hr
}