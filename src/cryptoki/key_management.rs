use std::{ptr, sync::Arc};

use rand::{rngs::OsRng, Rng};

use super::{
    bindings::{
        CKM_AES_KEY_GEN, CKR_ARGUMENTS_BAD, CKR_FUNCTION_NOT_SUPPORTED, CKR_OK, CK_ATTRIBUTE_PTR,
        CK_BYTE_PTR, CK_MECHANISM_PTR, CK_OBJECT_HANDLE, CK_OBJECT_HANDLE_PTR, CK_RV,
        CK_SESSION_HANDLE, CK_ULONG, CK_ULONG_PTR,
    },
    internals::encryption::{decrypt, destructure_iv_ciphertext, encrypt},
    utils::FromPointer,
};
use crate::state::{
    object::{
        cryptoki_object::CryptokiObject, private_key_object::PrivateKeyObject,
        secret_key_object::SecretKeyObject, template::Template,
    },
    StateAccessor,
};

pub(crate) type Aes128CbcEnc = cbc::Encryptor<aes::Aes128>;
pub(crate) type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;
pub(crate) const AES_BLOCK_SIZE: usize = 16;
pub(crate) const AES_IV_SIZE: usize = AES_BLOCK_SIZE;

/// Generates a secret key or set of domain parameters, creating a new object
///
/// # Arguments
///
/// * `hSession` - the session’s handle
/// * `pMechanism` - points to the generation mechanism
/// * `pTemplate` - points to the template for the new key or set of domain parameters
/// * `ulCount` - the number of attributes in the template
/// * `phKey` - points to the location that receives the handle of the new key or set of domain parameters
#[allow(non_snake_case)]
pub(crate) fn C_GenerateKey(
    hSession: CK_SESSION_HANDLE,
    pMechanism: CK_MECHANISM_PTR,
    pTemplate: CK_ATTRIBUTE_PTR,
    ulCount: CK_ULONG,
    phKey: CK_OBJECT_HANDLE_PTR,
) -> CK_RV {
    if pMechanism.is_null() || pTemplate.is_null() || phKey.is_null() {
        return CKR_ARGUMENTS_BAD as CK_RV;
    }

    let mechanism = unsafe { *pMechanism };
    // todo: implement others
    if mechanism.mechanism as u32 != CKM_AES_KEY_GEN {
        return CKR_FUNCTION_NOT_SUPPORTED as CK_RV;
    }
    let template = unsafe { Vec::from_pointer(pTemplate, ulCount as usize) };
    let template = Template::from(template);
    let mut object = SecretKeyObject::from_template(template);

    let key: [u8; 16] = OsRng.gen();
    object.store_value(key.into());

    let state_accessor = StateAccessor::new();
    let object_handle = match state_accessor.create_object(&hSession, Arc::new(object)) {
        Ok(handle) => handle,
        Err(err) => err.into_ck_rv(),
    };
    unsafe { *phKey = object_handle };

    CKR_OK as CK_RV
}

/// Generates a public/private key pair, creating new key objects
///
/// # Arguments
///
/// * `hSession` - the session’s handle
/// * `pMechanism` - points to the key generation mechanism
/// * `pPublicKeyTemplate` - points to the template for the public key
/// * `ulPublicKeyAttributeCount` - the number of attributes in the public-key template
/// * `pPrivateKeyTemplate` - points to the template for the private key
/// * `ulPrivateKeyAttributeCount` - the number of attributes in the private-key template
/// * `phPublicKey` - points to the location that receives the handle of the new public key
/// * `phPrivateKey` - points to the location that receives the handle of the new private key
#[allow(non_snake_case)]
pub(crate) fn C_GenerateKeyPair(
    hSession: CK_SESSION_HANDLE,
    pMechanism: CK_MECHANISM_PTR,
    pPublicKeyTemplate: CK_ATTRIBUTE_PTR,
    ulPublicKeyAttributeCount: CK_ULONG,
    pPrivateKeyTemplate: CK_ATTRIBUTE_PTR,
    ulPrivateKeyAttributeCount: CK_ULONG,
    phPublicKey: CK_OBJECT_HANDLE_PTR,
    phPrivateKey: CK_OBJECT_HANDLE_PTR,
) -> CK_RV {
    let state_accessor = StateAccessor::new();
    let (private_key_handle, pubkey_handle) = match state_accessor.get_keypair(&hSession) {
        Ok(val) => val,
        Err(err) => return err.into_ck_rv(),
    };

    unsafe {
        *phPublicKey = pubkey_handle;
        *phPrivateKey = private_key_handle;
    };

    CKR_OK as CK_RV
}

/// Wraps (i.e., encrypts) a private or secret key
///
/// # Arguments
///
/// * `hSession` - the session’s handle
/// * `pMechanism` - points to the wrapping mechanism
/// * `hWrappingKey` - the handle of the wrapping key
/// * `hKey` - the handle of the key to be wrapped
/// * `pWrappedKey` - points to the location that receives the wrapped key
/// * `pulWrappedKeyLen` - points to the location that receives the length of the wrapped key
#[allow(non_snake_case)]
pub(crate) fn C_WrapKey(
    hSession: CK_SESSION_HANDLE,
    pMechanism: CK_MECHANISM_PTR,
    hWrappingKey: CK_OBJECT_HANDLE,
    hKey: CK_OBJECT_HANDLE,
    pWrappedKey: CK_BYTE_PTR,
    pulWrappedKeyLen: CK_ULONG_PTR,
) -> CK_RV {
    if pulWrappedKeyLen.is_null() {
        return CKR_ARGUMENTS_BAD as CK_RV;
    }
    let state_accessor = StateAccessor::new();
    let wrapping_key = match state_accessor.get_object(&hSession, &hWrappingKey) {
        Ok(val) => val,
        Err(err) => return err.into_ck_rv(),
    };
    let private_key = match state_accessor.get_object(&hSession, &hKey) {
        Ok(val) => val,
        Err(err) => return err.into_ck_rv(),
    };
    let private_key = private_key.get_value().unwrap();
    let key = &wrapping_key.get_value().unwrap();

    let encryption_output = encrypt(key, private_key);
    let ciphertext_with_iv = encryption_output.into_combined();
    unsafe {
        *pulWrappedKeyLen = ciphertext_with_iv.len() as CK_ULONG;
    }

    if pWrappedKey.is_null() {
        return CKR_OK as CK_RV;
    }

    unsafe {
        ptr::copy(
            ciphertext_with_iv.as_ptr(),
            pWrappedKey,
            ciphertext_with_iv.len(),
        );
    }

    // TODO: either buffer ciphertext length or only precompute it if pWrappedKey is null
    // now encryption is done twice
    CKR_OK as CK_RV
}

/// Unwraps (i.e. decrypts) a wrapped key, creating a new private key or secret key object
///
/// # Arguments
///
/// * `hSession` - the session’s handle
/// * `pMechanism` - points to the unwrapping mechanism
/// * `hUnwrappingKey` - the handle of the unwrapping key
/// * `pWrappedKey` - points to the wrapped key
/// * `ulWrappedKeyLen` - the length of the wrapped key
/// * `pTemplate` - points to the template for the new key
/// * `ulAttributeCount` - the number of attributes in the template
/// * `phKey` - points to the location that receives the handle of the recovered key
#[allow(non_snake_case)]
pub(crate) fn C_UnwrapKey(
    hSession: CK_SESSION_HANDLE,
    pMechanism: CK_MECHANISM_PTR,
    hUnwrappingKey: CK_OBJECT_HANDLE,
    pWrappedKey: CK_BYTE_PTR,
    ulWrappedKeyLen: CK_ULONG,
    pTemplate: CK_ATTRIBUTE_PTR,
    ulAttributeCount: CK_ULONG,
    phKey: CK_OBJECT_HANDLE_PTR,
) -> CK_RV {
    if pWrappedKey.is_null() {
        return CKR_ARGUMENTS_BAD as CK_RV;
    }

    let state_accessor = StateAccessor::new();
    let unwrapping_key = match state_accessor.get_object(&hSession, &hUnwrappingKey) {
        Ok(val) => val,
        Err(err) => return err.into_ck_rv(),
    };

    let key = unwrapping_key.get_value().unwrap();
    let encryption_output =
        unsafe { destructure_iv_ciphertext(pWrappedKey, ulWrappedKeyLen as usize) };

    let plaintext = decrypt(&key, encryption_output.ciphertext, encryption_output.iv);

    // TODO: create from template
    let mut private_key_object = PrivateKeyObject::new();
    private_key_object.store_value(plaintext);

    let handle =
        match state_accessor.create_ephemeral_object(&hSession, Arc::new(private_key_object)) {
            Ok(val) => val,
            Err(err) => return err.into_ck_rv(),
        };
    unsafe {
        *phKey = handle;
    }
    CKR_OK as CK_RV
}
