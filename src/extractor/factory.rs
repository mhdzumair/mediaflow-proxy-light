//! Extractor registry — maps host name strings to extractor instances.

use std::collections::HashMap;

use crate::extractor::{
    base::{Extractor, ExtractorError},
    hosts::{
        city::CityExtractor, doodstream::DoodStreamExtractor, f16px::F16PxExtractor,
        fastream::FastreamExtractor, filelions::FileLionsExtractor, filemoon::FileMoonExtractor,
        gupload::GuploadExtractor, livetv::LiveTVExtractor, lulustream::LuluStreamExtractor,
        maxstream::MaxstreamExtractor, mixdrop::MixdropExtractor, okru::OkruExtractor,
        sportsonline::SportsonlineExtractor, streamtape::StreamtapeExtractor,
        streamwish::StreamWishExtractor, supervideo::SupervideoExtractor,
        turbovidplay::TurboVidPlayExtractor, uqload::UqloadExtractor, vavoo::VavooExtractor,
        vidfast::VidFastExtractor, vidmoly::VidmolyExtractor, vidoza::VidozaExtractor,
        vixcloud::VixCloudExtractor, voe::VoeExtractor,
    },
};

type BoxExtractor = Box<dyn Extractor>;
type FactoryFn = fn(HashMap<String, String>, Option<String>) -> BoxExtractor;

/// Returns an extractor instance for the given `host` name (case-insensitive),
/// forwarding the caller-supplied `request_headers` and `proxy_url`.
pub fn get_extractor(
    host: &str,
    request_headers: HashMap<String, String>,
    proxy_url: Option<String>,
) -> Result<BoxExtractor, ExtractorError> {
    macro_rules! extractors {
        ($($name:expr => $ty:ty),* $(,)?) => {{
            static MAP: std::sync::OnceLock<HashMap<&'static str, FactoryFn>> =
                std::sync::OnceLock::new();
            let m = MAP.get_or_init(|| {
                let mut m: HashMap<&'static str, FactoryFn> = HashMap::new();
                $(m.insert($name, |h, p| Box::new(<$ty>::new(h, p)));)*
                m
            });
            m
        }};
    }

    let registry = extractors!(
        "city"         => CityExtractor,
        "doodstream"   => DoodStreamExtractor,
        "filelions"    => FileLionsExtractor,
        "filemoon"     => FileMoonExtractor,
        "f16px"        => F16PxExtractor,
        "gupload"      => GuploadExtractor,
        "uqload"       => UqloadExtractor,
        "mixdrop"      => MixdropExtractor,
        "streamtape"   => StreamtapeExtractor,
        "streamwish"   => StreamWishExtractor,
        "supervideo"   => SupervideoExtractor,
        "turbovidplay" => TurboVidPlayExtractor,
        "vixcloud"     => VixCloudExtractor,
        "okru"         => OkruExtractor,
        "maxstream"    => MaxstreamExtractor,
        "livetv"       => LiveTVExtractor,
        "lulustream"   => LuluStreamExtractor,
        "vavoo"        => VavooExtractor,
        "vidmoly"      => VidmolyExtractor,
        "vidoza"       => VidozaExtractor,
        "fastream"     => FastreamExtractor,
        "voe"          => VoeExtractor,
        "sportsonline" => SportsonlineExtractor,
        "vidfast"      => VidFastExtractor,
    );

    let key = host.to_lowercase();
    let factory = registry
        .get(key.as_str())
        .ok_or_else(|| ExtractorError::extract(format!("Unsupported host: {host}")))?;

    Ok(factory(request_headers, proxy_url))
}
