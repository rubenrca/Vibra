use vibra_remote::*;
fn main() -> anyhow::Result<()> {
    let a = [1u8; 32];
    let b = [2u8; 32];
    let ae = [3u8; 32];
    let be = [4u8; 32];
    // Derive fixed fixture public keys through the same Noise primitive resolver.
    let params = PATTERN.parse()?;
    let mut dh = snow::resolvers::CryptoResolver::resolve_dh(
        &snow::resolvers::DefaultResolver,
        &snow::params::DHChoice::Curve25519,
    )
    .unwrap();
    dh.set(&b);
    let bp = dh.pubkey().to_vec();
    let mut i = snow::Builder::new(params)
        .local_private_key(&a)?
        .remote_public_key(&bp)?
        .prologue(PROLOGUE)?
        .fixed_ephemeral_key_for_testing_only(&ae)
        .build_initiator()?;
    let mut r = snow::Builder::new(PATTERN.parse()?)
        .local_private_key(&b)?
        .prologue(PROLOGUE)?
        .fixed_ephemeral_key_for_testing_only(&be)
        .build_responder()?;
    let mut buf = vec![0; WIRE_LIMIT];
    let mut plain = vec![0; WIRE_LIMIT];
    let n = i.write_message(br#"{"invitation":"fixture","name":"iPhone"}"#, &mut buf)?;
    let first = buf[..n].to_vec();
    r.read_message(&first, &mut plain)?;
    let n = r.write_message(b"approved", &mut buf)?;
    let second = buf[..n].to_vec();
    i.read_message(&second, &mut plain)?;
    let mut i = Channel::new(i.into_transport_mode()?);
    let mut r = Channel::new(r.into_transport_mode()?);
    let message = "Español 日本語 🦀\u{1b}[31m".repeat(6000);
    let host = r
        .seal(message.as_bytes())?
        .iter()
        .map(|b| base64(b))
        .collect::<Vec<_>>();
    let phone = i
        .seal(b"Ctrl+C")?
        .iter()
        .map(|b| base64(b))
        .collect::<Vec<_>>();
    println!(
        "{}",
        serde_json::json!({"private":base64(&a),"public":base64(&bp),"ephemeral":base64(&ae),"first":base64(&first),"second":base64(&second),"host":host,"phone":phone,"message":message})
    );
    Ok(())
}
