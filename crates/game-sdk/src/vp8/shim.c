/* libvpx の VP8 の圧縮・復号を、Rust から struct の配置に触れずに呼ぶための薄い関数（ADR 0070 §8.3、#1982）。 */
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "vpx/vp8cx.h"
#include "vpx/vp8dx.h"
#include "vpx/vpx_decoder.h"
#include "vpx/vpx_encoder.h"

typedef struct {
  vpx_codec_ctx_t codec;
  vpx_image_t image;
  vpx_codec_iter_t iter;
} kvpx_encoder;

typedef struct {
  vpx_codec_ctx_t codec;
  vpx_codec_iter_t iter;
  vpx_image_t *image;
} kvpx_decoder;

/* realtime・CBR・先読み 0・cpu-used 10（ADR 0070 §8.3）。時刻の単位は μs。 */
kvpx_encoder *kvpx_encoder_new(uint32_t width, uint32_t height, uint32_t kbps, uint32_t threads) {
  vpx_codec_enc_cfg_t cfg;
  kvpx_encoder *encoder;
  if (vpx_codec_enc_config_default(vpx_codec_vp8_cx(), &cfg, 0) != VPX_CODEC_OK) return NULL;
  cfg.g_w = width;
  cfg.g_h = height;
  cfg.g_timebase.num = 1;
  cfg.g_timebase.den = 1000000;
  cfg.g_threads = threads;
  cfg.g_lag_in_frames = 0;
  cfg.g_error_resilient = VPX_ERROR_RESILIENT_DEFAULT;
  cfg.rc_end_usage = VPX_CBR;
  cfg.rc_target_bitrate = kbps;
  cfg.kf_mode = VPX_KF_DISABLED;
  encoder = calloc(1, sizeof(*encoder));
  if (encoder == NULL) return NULL;
  if (vpx_codec_enc_init(&encoder->codec, vpx_codec_vp8_cx(), &cfg, 0) != VPX_CODEC_OK) {
    free(encoder);
    return NULL;
  }
  vpx_codec_control(&encoder->codec, VP8E_SET_CPUUSED, 10);
  if (vpx_img_alloc(&encoder->image, VPX_IMG_FMT_I420, width, height, 1) == NULL) {
    vpx_codec_destroy(&encoder->codec);
    free(encoder);
    return NULL;
  }
  return encoder;
}

/* 詰めて並べた I420（Y・U・V の順）を圧縮する。出力は kvpx_encoder_packet で取り出す。 */
int kvpx_encode(kvpx_encoder *encoder, const uint8_t *i420, int64_t pts_us, int keyframe) {
  vpx_image_t *image = &encoder->image;
  unsigned int plane, row;
  const uint8_t *source = i420;
  for (plane = 0; plane < 3; plane++) {
    unsigned int width = plane == 0 ? image->d_w : (image->d_w + 1) / 2;
    unsigned int height = plane == 0 ? image->d_h : (image->d_h + 1) / 2;
    for (row = 0; row < height; row++) {
      memcpy(image->planes[plane] + (size_t)row * image->stride[plane], source, width);
      source += width;
    }
  }
  encoder->iter = NULL;
  return vpx_codec_encode(&encoder->codec, image, pts_us, 1, keyframe ? VPX_EFLAG_FORCE_KF : 0,
                          VPX_DL_REALTIME) == VPX_CODEC_OK
             ? 0
             : -1;
}

/* 次の圧縮済みの frame。無ければ NULL。 */
const uint8_t *kvpx_encoder_packet(kvpx_encoder *encoder, size_t *len, int *keyframe) {
  const vpx_codec_cx_pkt_t *packet;
  while ((packet = vpx_codec_get_cx_data(&encoder->codec, &encoder->iter)) != NULL) {
    if (packet->kind == VPX_CODEC_CX_FRAME_PKT) {
      *len = packet->data.frame.sz;
      *keyframe = (packet->data.frame.flags & VPX_FRAME_IS_KEY) != 0;
      return packet->data.frame.buf;
    }
  }
  return NULL;
}

void kvpx_encoder_free(kvpx_encoder *encoder) {
  vpx_img_free(&encoder->image);
  vpx_codec_destroy(&encoder->codec);
  free(encoder);
}

kvpx_decoder *kvpx_decoder_new(uint32_t threads) {
  vpx_codec_dec_cfg_t cfg = {0};
  kvpx_decoder *decoder = calloc(1, sizeof(*decoder));
  if (decoder == NULL) return NULL;
  cfg.threads = threads;
  if (vpx_codec_dec_init(&decoder->codec, vpx_codec_vp8_dx(), &cfg, 0) != VPX_CODEC_OK) {
    free(decoder);
    return NULL;
  }
  return decoder;
}

int kvpx_decode(kvpx_decoder *decoder, const uint8_t *data, size_t len) {
  decoder->iter = NULL;
  return vpx_codec_decode(&decoder->codec, data, (unsigned int)len, NULL, 0) == VPX_CODEC_OK ? 0 : -1;
}

/* 次の復号した frame の大きさ。無ければ 0。続く kvpx_decoder_copy がその frame を写す。 */
int kvpx_decoder_next(kvpx_decoder *decoder, uint32_t *width, uint32_t *height) {
  decoder->image = vpx_codec_get_frame(&decoder->codec, &decoder->iter);
  if (decoder->image == NULL) return 0;
  *width = decoder->image->d_w;
  *height = decoder->image->d_h;
  return 1;
}

/* kvpx_decoder_next の frame を、詰めた I420（Y・U・V の順）で `out` へ写す。`out` は大きさに足りる容量を持つ。 */
void kvpx_decoder_copy(kvpx_decoder *decoder, uint8_t *out) {
  vpx_image_t *image = decoder->image;
  unsigned int plane, row;
  for (plane = 0; plane < 3; plane++) {
    unsigned int w = plane == 0 ? image->d_w : (image->d_w + 1) / 2;
    unsigned int h = plane == 0 ? image->d_h : (image->d_h + 1) / 2;
    for (row = 0; row < h; row++) {
      memcpy(out, image->planes[plane] + (size_t)row * image->stride[plane], w);
      out += w;
    }
  }
}

void kvpx_decoder_free(kvpx_decoder *decoder) {
  vpx_codec_destroy(&decoder->codec);
  free(decoder);
}
