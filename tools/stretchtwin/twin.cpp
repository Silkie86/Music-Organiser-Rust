// The original Signalsmith Stretch (C++), run the way crates/mo-stretch/examples/raw.rs runs ours, so the two can be
// compared (run.sh). Same arguments: IN CHANNELS RATE IN_SAMPLES OUT_SAMPLES SEMITONES FORMANT_SEMITONES COMPENSATE OUT.
#include "signalsmith-stretch.h"
#include <cstdio>
#include <cstdlib>
#include <vector>

int main(int argc, char **argv) {
	if (argc != 10) return 1;
	int channels = atoi(argv[2]);
	float rate = atof(argv[3]);
	int inSamples = atoi(argv[4]), outSamples = atoi(argv[5]);
	float semitones = atof(argv[6]), formant = atof(argv[7]);
	bool compensate = atoi(argv[8]) != 0;
	std::vector<std::vector<float>> in(channels, std::vector<float>(inSamples)), out(channels, std::vector<float>(outSamples));
	FILE *f = fopen(argv[1], "rb");
	if (!f) return 1;
	for (auto &c : in) {
		if (fread(c.data(), 4, inSamples, f) != size_t(inSamples)) return 1;
	}
	fclose(f);
	signalsmith::stretch::SignalsmithStretch<float> stretch(42);
	stretch.presetDefault(channels, rate);
	if (semitones != 0) stretch.setTransposeSemitones(semitones);
	if (formant != 0 || compensate) stretch.setFormantSemitones(formant, compensate);
	stretch.exact(in, inSamples, out, outSamples);
	f = fopen(argv[9], "wb");
	if (!f) return 1;
	for (auto &c : out) fwrite(c.data(), 4, outSamples, f);
	fclose(f);
	return 0;
}
