"""Checks the theory bench's chord scores against mir_eval itself.

Usage: python3 -I check_mir_eval.py <dump.jsonl>
  (the file `theorybench chords ... --dump <dump.jsonl>` writes; needs `pip install mir_eval`)

For every engine and piece it runs mir_eval's own steps (adjust_intervals, merge_labeled_intervals, the comparison
functions and directional_hamming_distance) on the same labels and chords the bench scored, then prints the pooled
scores both ways and the pieces where they differ most. Any difference larger than rounding is a bug in the bench.
The times are in each piece's ticks; songs are pooled in beats (ticks over the piece's ticks a beat), as the bench
pools them.
"""
import json
import sys

import mir_eval
import numpy as np

MEASURES = {
    'root': 'root',
    'majmin': 'majmin',
    'majmin+bass': 'majmin_inv',
    'thirds': 'thirds',
    'triads': 'triads',
    'sevenths': 'sevenths',
    '7ths+bass': 'sevenths_inv',
    'tetrads': 'tetrads',
    'mirex': 'mirex',
}


def score(reference, estimate):
    ref_int = np.array([[s, e] for s, e, _ in reference], dtype=float)
    ref_lab = [label for _, _, label in reference]
    est_int = np.array([[s, e] for s, e, _ in estimate], dtype=float).reshape(-1, 2)
    est_lab = [label for _, _, label in estimate]
    est_int, est_lab = mir_eval.util.adjust_intervals(
        est_int, est_lab, ref_int.min(), ref_int.max(), mir_eval.chord.NO_CHORD, mir_eval.chord.NO_CHORD)
    merged_ref = mir_eval.chord.merge_chord_intervals(ref_int, ref_lab)
    merged_est = mir_eval.chord.merge_chord_intervals(est_int, est_lab)
    intervals, ref_lab, est_lab = mir_eval.util.merge_labeled_intervals(ref_int, ref_lab, est_int, est_lab)
    durations = mir_eval.util.intervals_to_durations(intervals)
    out = {}
    for ours, theirs in MEASURES.items():
        comparison = getattr(mir_eval.chord, theirs)(ref_lab, est_lab)
        valid = comparison >= 0
        out[ours] = (float(np.sum(durations[valid] * comparison[valid])), float(np.sum(durations[valid])))
    # directional_hamming_distance divides by its first argument's stretch (first start to last end, gaps and all);
    # multiplying back gives the length lost, so songs can be pooled the way the bench pools them.
    over_total = float(merged_ref[-1, 1] - merged_ref[0, 0])
    under_total = float(merged_est[-1, 1] - merged_est[0, 0])
    over = mir_eval.chord.directional_hamming_distance(merged_ref, merged_est) * over_total
    under = mir_eval.chord.directional_hamming_distance(merged_est, merged_ref) * under_total
    return out, (over, over_total, under, under_total)


def main():
    pooled = {}
    worst = []
    for line in open(sys.argv[1], encoding='utf-8'):
        d = json.loads(line)
        theirs, theirs_seg = score(d['reference'], d['estimate'])
        ours = dict((name, (right, scored)) for name, (right, scored) in d['measures'])
        beat = max(1, d.get('ppq', 1))
        p = pooled.setdefault(d['engine'], {m: [0.0, 0.0, 0.0, 0.0] for m in MEASURES} | {'seg': [0.0] * 8})
        for m in MEASURES:
            p[m][0] += ours[m][0] / beat
            p[m][1] += ours[m][1] / beat
            p[m][2] += theirs[m][0] / beat
            p[m][3] += theirs[m][1] / beat
            a = ours[m][0] / ours[m][1] if ours[m][1] else 0.0
            b = theirs[m][0] / theirs[m][1] if theirs[m][1] else 0.0
            worst.append((abs(a - b), d['engine'], d['id'], m, a, b))
        # Lost and total, over-cut then under-cut: the bench's four numbers, then mir_eval's.
        for i, v in enumerate(list(d['segmentation']) + list(theirs_seg)):
            p['seg'][i] += v / beat
    for engine, p in pooled.items():
        print(engine)
        for m in MEASURES:
            r, t, mr, mt = p[m]
            print(f'  {m:12} bench {100 * r / t:7.3f}%   mir_eval {100 * mr / mt:7.3f}%   (beats scored {t:.0f} / {mt:.0f})')
        lo, to, lu, tu, mo, mto, mu, mtu = p['seg']
        print(f'  over-cut     bench {100 * lo / to:7.3f}    mir_eval {100 * mo / mto:7.3f}')
        print(f'  under-cut    bench {100 * lu / tu:7.3f}    mir_eval {100 * mu / mtu:7.3f}')
    worst.sort(reverse=True)
    print('Largest differences in one piece:')
    for diff, engine, sid, m, a, b in worst[:10]:
        print(f'  {engine} {sid} {m}: bench {100 * a:.3f}% mir_eval {100 * b:.3f}% ({100 * diff:.3f} points)')


if __name__ == '__main__':
    main()
