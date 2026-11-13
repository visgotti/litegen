/* probe_sequences.c: the offsets libzstd itself chooses, for the README's census.
 *
 * ktx2.rs reads the sequences of every file in this folder with a decoder of its own and reports
 * how far back the matches reach. This program asks libzstd (ZSTD_generateSequences, an
 * experimental API) what its compressor chooses for the same input and level, so the two can be
 * compared. It is not part of the tests and is not built by anything.
 *
 *   cc -O1 -I/opt/homebrew/include -o probe_sequences probe_sequences.c -L/opt/homebrew/lib -lzstd
 *   ./probe_sequences <input.bin> <level> [<windowLog> [nopledge]]
 *
 *   <input.bin>  one of the inputs make_corpus.mjs writes to <scratch>/in
 *   <level>      the compression level (3, 19, 22, ...)
 *   <windowLog>  0 for the default; otherwise the window, and long-distance matching turned on,
 *                which is what `zstd --long=27` does
 *   nopledge     do not tell the compressor the input's size, as `zstd < input` does not
 *
 * Prints the sequence count, the largest offset and how many bytes the sequences cover.
 */
#define ZSTD_STATIC_LINKING_ONLY
#include <stdio.h>
#include <stdlib.h>
#include <zstd.h>

int main(int argc, char **argv) {
  if (argc < 3) {
    fprintf(stderr, "usage: %s <input.bin> <level> [<windowLog> [nopledge]]\n", argv[0]);
    return 2;
  }
  FILE *f = fopen(argv[1], "rb");
  if (!f) return 2;
  fseek(f, 0, SEEK_END);
  long n = ftell(f);
  fseek(f, 0, SEEK_SET);
  char *src = malloc(n);
  if (fread(src, 1, n, f) != (size_t)n) return 2;
  fclose(f);
  int level = atoi(argv[2]);
  int window_log = argc > 3 ? atoi(argv[3]) : 0;
  ZSTD_CCtx *cc = ZSTD_createCCtx();
  ZSTD_CCtx_setParameter(cc, ZSTD_c_compressionLevel, level);
  if (window_log) {
    ZSTD_CCtx_setParameter(cc, ZSTD_c_windowLog, window_log);
    ZSTD_CCtx_setParameter(cc, ZSTD_c_enableLongDistanceMatching, ZSTD_ps_enable);
  }
  if (argc < 5) ZSTD_CCtx_setPledgedSrcSize(cc, n);
  size_t capacity = ZSTD_sequenceBound(n);
  ZSTD_Sequence *seqs = malloc(capacity * sizeof(ZSTD_Sequence));
  size_t count = ZSTD_generateSequences(cc, seqs, capacity, src, n);
  if (ZSTD_isError(count)) {
    fprintf(stderr, "error: %s\n", ZSTD_getErrorName(count));
    return 1;
  }
  unsigned largest = 0;
  size_t covered = 0;
  for (size_t i = 0; i < count; i++) {
    if (seqs[i].offset > largest) largest = seqs[i].offset;
    covered += seqs[i].litLength + seqs[i].matchLength;
  }
  printf("sequences %zu, largest offset %u, covers %zu of %ld\n", count, largest, covered, n);
  return 0;
}
