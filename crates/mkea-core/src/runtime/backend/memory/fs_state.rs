// Owned filesystem/resource state extracted from the monolithic backend file.

#[derive(Debug, Clone, Default)]
pub(crate) struct FsState {
    bundle_root: Option<PathBuf>,
    bundle_resource_index: HashMap<String, PathBuf>,
    resource_image_cache: HashMap<String, u32>,
    bundle_roots: HashMap<u32, PathBuf>,
    host_files: HashMap<u32, HostFileHandle>,
    synthetic_file_urls: HashMap<u32, SyntheticFileUrlState>,
    synthetic_data_providers: HashMap<u32, SyntheticDataProviderState>,
    synthetic_audio_files: HashMap<u32, SyntheticAudioFileState>,
    bundle_objects_created: u32,
    bundle_scoped_hits: u32,
    bundle_scoped_misses: u32,
    png_cgbi_detected: u32,
    png_cgbi_decoded: u32,
    png_decode_failures: u32,
    image_named_hits: u32,
    image_named_misses: u32,
    file_open_hits: u32,
    file_open_misses: u32,
    file_read_ops: u32,
    file_bytes_read: u32,
    last_resource_name: Option<String>,
    last_resource_path: Option<String>,
    last_file_path: Option<String>,
    last_file_mode: Option<String>,
}
