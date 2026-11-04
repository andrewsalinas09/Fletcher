# Targets and preference bounds: how squig.link does it, and what's licensed

Researched 2026-11-04 for Q-31 (issue #13). Sources were read from the code (repos cloned) and live site scripts, not from descriptions. Where a fact couldn't be verified, it says so.

## What squig.link is

- A database of headphone measurements run by reviewers ("squigglers"): Lab (the site), Server (API), Studio (for creators). Each reviewer has a subdomain (`<name>.squig.link`). Sources: [squiglink docs](https://github.com/squiglink/documentation/blob/main/source/overview/getting-started.md), [squiglink/lab](https://github.com/squiglink/lab).
- Lab is a fork of Marshall Lochbaum's CrinGraph, built for crinacle.com ([CrinGraph docs](https://mlochbaum.github.io/CrinGraph/Documentation.html)).
- Tilt controls and preference bounds are not in squiglink/lab. They're in HarutoHiroki's [PublicGraphTool](https://github.com/HarutoHiroki/PublicGraphTool), a fork many sites run (for example listener.squig.link).
- Not verified: who legally operates squig.link.

## Targets and how they're lined up

- Every site picks its own targets in `config.js`. Groups seen: Neutral (KEMAR DF, IEF Neutral 2023…), Reviewer targets, Preference (Harman IE 2019v2, Harman 2018 OE…). Rig-specific sets exist, for example listener.squig.link/5128 has "5128 DF", "ISO 11904-1 DF" and "Harman IE 2019 for 5128" ([config](https://listener.squig.link/assets/js/config.js), [5128 config](https://listener.squig.link/assets/js/config_5128.js)).
- Normalization, two modes:
  - "dB" (default): each curve is shifted so its ISO 226-weighted pink-noise loudness is 60 dB, using the whole curve.
  - "Hz": the response at one frequency is set to 60 dB. Lab defaults to 500 Hz ("recommended by IEC"); PublicGraphTool and Listener's sites use 630 Hz.
  - Curves are smoothed before normalizing, and L/R are normalized on their average.
- Target adjustments (PublicGraphTool `graphtool.js`, `updateDF`). They apply only to diffuse-field base targets ("tiltable"):
  - Bass: low shelf, 105 Hz, Q 0.707.
  - Ear gain: peak, 2750 Hz, Q 1.
  - Treble: high shelf, 2500 Hz, Q 0.42.
  - Tilt: `tilt × log2(f)` dB, the slider in dB/octave; normalization then removes the overall offset.
- The Harman curves are expressed as presets of those knobs on top of a DF target. For example "Harman 2018 OE" is bass +6.6, ear −1.8, treble −3, tilt 0 in the demo config (Listener's config: +6 / −1 / −4). **So a target can be "a base response plus four numbers"**, which is a natural shape for user-made targets.

## Preference bounds

- **Author:** Griffin Silver ("Listener") at Headphones.com ([article](https://headphones.com/blogs/features/what-headphone-sound-profiles-do-people-like)).
- **What they are:** a band of variation built from Harman listener-preference data, instead of a single target line.
- **Data:**
  - 2023 version: Harman's 2013 paper "Listener Preferences for In-Room Loudspeaker and Headphone Target Responses" (mean preferred adjustments with upper/lower confidence intervals).
  - Newer over-ear version: Harman's 2018 over-ear study plus the 2019 segmentation paper ("Class 1"); preferred curves replicated on two fixtures, smoothed, outermost 10% rejected.
  - In-ear (beta): Olive's in-ear work, 2016–2025.
  - Confirmed as "derived from the existing speaker and headphone literature from Harman" ([forum](https://forum.headphones.com/t/understanding-the-headphones-com-iem-measurements/23607)).
- **Scope:** the over-ear bounds are "meant solely to be used with Diffuse Field compensated measurements of over-ear headphones" (GRAS Type 3.3 and 5128 Type 4.3).
- **How they're drawn:** two curves (`Bounds L.txt`, `Bounds R.txt`, used only as two edges) are dB offsets added to the active DF base target, then normalized and smoothed, and shown as a shaded band (`prefBounds.js`). modernGraphTool documents the same idea: upper/lower offsets from the base target, roughly 6 dB wide at 50 Hz and 2 dB at 500 Hz ([docs](https://potatosalad775.github.io/modernGraphTool/docs/features/preference-bound)).
- Not verified: the original wording in the Headphones.com 5128 guide (the fetch was rate-limited).

## Rigs

Targets are rig-specific. Sites handle it with a rig label per site (`rig_description`, e.g. "clone IEC 711"), separate databases and targets per rig, and the "5128Δ" project, which compensates each site's coupler toward a reference because "the same earphone measured by different Squiglink sites can (and usually does) result in different graphs… the hardware microphones (711 couplers) we use are surprisingly variable" ([intro](https://squig.link/squiglink-intro.js)).

## File format

REW "Export measurement as text": 20–20,000 Hz, 48 points per octave, tab-separated, frequency then dB. Measurements are `Name L.txt` / `Name R.txt` (`L1.txt`, `L2.txt`… for several seatings); targets are `Name Target.txt`; an index `phone_book.json` lists models ([docs](https://github.com/squiglink/documentation/blob/main/source/squigglers/managing-a-database.md)). Fletcher already reads this family of files (REW import, ADR-0016's parsers).

## Licensing

- **Code:** squiglink lab/server/studio/docs and CrinGraph are 0BSD; PublicGraphTool is MIT (its README also asks for credit to the original author).
- **Data:** no license or terms of use found for any measurement, target or bounds data on any site or repo. CrinGraph's docs say its bundled targets "were provided by Crinacle so make sure you have his permission before using", and that Crinacle's measurements "are not public". **Treat squig.link data and the Headphones.com bounds as all rights reserved.**
- Consequence for Fletcher: borrow the *ideas* (knob-based targets, bounds as offset curves around a base, normalization modes), not the data. Built-in bounds would need permission or a derivation of our own from the published Harman papers.

## PEQdB

- A website and company, not a single target: its tool is at peqdb.com/studio ([Sierra Softworks](https://blog.sierrasoftworks.com/eq-ing-headphones-with-peqdb/)). Founder Saurav Chala (Stanford CCRMA). It runs its own blind listening tests: Gaussian-process EQ ratings over 40 trials, then bass / ear-gain / treble sliders (13 dB range each), inconsistent listeners filtered out, LUFS loudness normalization ([CCRMA talk](https://ccrma.stanford.edu/events/saurav-chala-peqdb-frequency-response-preference-headphones)).
- It has its own targets: "HiFiEndgame" (won its blind shootout: more bass and treble, a deeper ear-gain dip near 2.7 kHz) and "PEQdB Diamond" (more bass and more treble above 6 kHz than Harman OE 2018).
- Its measurements are a scrape of squig.link ([IEM EQ Guide](https://iem-eq-guide.pages.dev/measurement-databases)), 711 rigs (one listing also claims 5128).
- Not verified: its license, export format or API (the site renders client-side). **Treat as unlicensed.** Its method is close to Fletcher's own (blind, level-matched preference); worth contacting.

## AutoEq: targets and measurements

- **Targets** (`targets/`, 61 CSVs): Harman over-ear 2013/2015/2018 and in-ear 2016–2019 (each also "without bass"), diffuse field 5128 / GRAS KEMAR / ISO 11904-1, "Diffuse field 5128 -1dB per octave", AutoEq in-ear, oratory1990 targets, LMG 5128 0.6, JM-1 variants, rig-specific variants (HMS II.3, crinacle EARS + 711, Rtings…), "711 5128 delta", zero ([listing](https://github.com/jaakkopasanen/AutoEq/tree/master/targets)).
- **Format:** `frequency,raw`, 20 Hz to ~19955 Hz, log grid stepping ×1.01 (~695 rows), matching `DEFAULT_F_MIN/F_MAX/STEP` in [constants.py](https://raw.githubusercontent.com/jaakkopasanen/AutoEq/master/autoeq/constants.py).
- **Pairing** ([create_webapp_data.py](https://github.com/jaakkopasanen/AutoEq/blob/master/dbtools/create_webapp_data.py)): each target lists the `{source, form, rig}` it's compatible with and a bass boost `{fc 105, q 0.7, gain}` applied to the "without bass" base. Harman OE 2018: +6 dB, GRAS 43AG-7 (crinacle). Harman IE 2019: +9.5, 711 / GRAS RA0045. LMG 5128 0.6: +6, Rtings B&K 5128. JM-1: +6.5, B&K 4620/5128. DF 5128 −1 dB/oct and flat: 0.
- **Measurements:** `measurements/<source>/data/<form>/[<rig>/]<model>.csv` (form: over-ear, in-ear, earbud), raw only, same format and grid; compensation happens at run time. Each source has `name_index.tsv`. Example: [oratory1990 HD 650](https://raw.githubusercontent.com/jaakkopasanen/AutoEq/master/measurements/oratory1990/data/over-ear/Sennheiser%20HD%20650.csv).
- **Changes:** many squig.link-crawled sources added (last commit 2025-07-20); `measurements/crinacle/` now has only its index, no data (reason not confirmed).
- **License:** MIT (© 2018–2022 Jaakko Pasanen), no separate data terms. The measurements come from third parties (oratory1990, crinacle, Rtings, squig sites), so redistribution rights are theirs, not AutoEq's.
- **Knobs** in AutoEq's CLI ([__main__.py](https://raw.githubusercontent.com/jaakkopasanen/AutoEq/master/autoeq/__main__.py)): `--bass-boost GAIN[,FC,Q]` (low shelf, default 105 Hz, Q 0.7), `--treble-boost` (high shelf, default 10 kHz, Q 0.7), `--tilt` dB/octave, `--sound-signature CSV` added to the target, plus max gain, treble limits and smoothing.

## Harman research

- In-ear bass: Olive/Welti/Khonsaripour 2016 ([AES 18369](https://www.aes.org/e-lib/browse.cfm?elib=18369)). In-ear preference models: AES 143 (2017), papers 9840/9878, predicting ratings from deviation from the target (r = 0.91).
- Over-ear preference model: AES 144 (2018), predicting from the standard deviation and absolute slope of the error curve (r = 0.86) ([Acoustics Today](https://acousticstoday.org/issues/2022AT/Spring2022/files/basic-html/page65.html)).
- Listener segments: AES paper 10156 (2019). Secondary sources: about 64% prefer Harman, about 21% want 2–3 dB less bass, about 15% want 4–6 dB more. Paper not read directly.
- **Harman publishes no "preference bounds".** The term and the band are Headphones.com's construction from Harman's data.
- Not verified: a primary source defining the "in-ear 2019" curve itself.

## Lining curves up, smoothing, left/right

- **Why 500 Hz:** IEC 60268-7:2010 §8.3.3.1: "The frequency of 500 Hz is chosen to avoid the effects of diaphragm resonance, leakage and standing waves that may occur in the coupler or ear simulator at other frequencies"; its subjective method references the 1/3-octave band at 500 Hz ([full text, national adoption](https://psqca.com.pk/cs/newitems2021/electronics/29-PS%20IEC%2060268-7-2015_FINAL.pdf)). Harman's targets are shown "normalized at 500 Hz" ([Olive, Acoustics Today 2022](https://acousticstoday.org/wp-content/uploads/2022/03/The-Perception-and-Measurement-of-Headphone-Sound-Quality-What-Do-Listeners-Prefer-Sean-E.-Olive.pdf)); its preference model uses the error curve's standard deviation and absolute slope, which ignore a constant offset. Not checked: whether the 2025 edition changes this.
- **AutoEq** ([frequency_response.py](https://github.com/jaakkopasanen/AutoEq/blob/master/autoeq/frequency_response.py)): centers at 1 kHz, then with `min_mean_error` (on in published results and the web app) shifts the *target* by the mean error over 100 Hz–10 kHz: a least-squares constant offset on the log grid. 100 Hz–10 kHz is the range the 711 ear simulator is specified for (IEC 60318-4: above 10 kHz "the device does not simulate a human ear").
- **Treble reliability** (studies reviewed in [Paquier & Koehl](https://www.bruit.fr/revues/78_13202.PDF)): reseating changes the response < 5 dB below 2 kHz but 8–15 dB above 4 kHz (Toole); SD < 2.5 dB below 10 kHz, up to 9 dB above (McAnally & Martin 2002); 9 dB SD at 9–14 kHz across placements (Kulkarni & Colburn 2000).
- **Smoothing:** REW offers 1/1–1/48, variable, psychoacoustic (a cubic mean that weights peaks) and ERB ([REW help](https://www.roomeqwizard.com/help/help_en-GB/html/graph.html)). AutoEq: Savitzky–Golay, 1/12 octave, blending to a 2-octave window between 6 and 8 kHz ([constants.py](https://github.com/jaakkopasanen/AutoEq/blob/master/autoeq/constants.py)); a known side effect is uncorrected 8–9 kHz peaks ([#706](https://github.com/jaakkopasanen/AutoEq/issues/706)). CrinGraph's smoother is index-based, with no psychoacoustic basis.
- **Left/right:** CrinGraph uses one offset from the average of both channels, so an imbalance stays visible; AutoEq's crawlers average L/R into one curve. Normalizing each channel separately would hide a real channel mismatch.
- **Recommendation (for the design):** anchor the measurement on the 1/3-octave band at 500 Hz; move the target by the least-squares offset over 100 Hz–10 kHz (option: median); never refit the predicted response (it must show the EQ's real gain and preamp); report the error's SD and slope; display 1/12 octave by default, with above 10 kHz marked as low confidence, and offer none / 1/24 / 1/12 / 1/6 / 1/3 / ERB; fit with AutoEq's defaults; one shared L/R offset, both channels shown, fit to the average by default with a per-ear option. Not verified: IEC 60318-4's "≈10 dB above 10 kHz" note; the Harman papers' own error band and smoothing.

## Where a measurement stops being reliable (researched 2026-11-04)

There is no single cutoff; reliability fades.
- **Reseating and individual differences:** Møller et al. 1995 (JAES 43(4); 14 headphones, 40 people; [full text](https://vbn.aau.dk/ws/files/227875204/1995_M_ller_et_al_AES_Journal_b.pdf)): repeated fittings "agree very well in most of the frequency range" but differ "especially at high frequencies… due to different positions of the characteristic dips". Between people, below 2 kHz mostly a level offset; at high frequencies SD "on the order of 3–5 dB", some differences of 20 dB; one headphone's dip falls anywhere from 7.5 to 10 kHz. At the blocked ear canal, a "very clear common structure up to at least 7 kHz", a common shape "still possible to observe" from 7 to 12 kHz, and "impossible" above 12 kHz.
- **Rigs:** the 711 coupler is specified to 10 kHz (IEC 60318-4), and Knowles says it "cannot accurately show the output level beyond about 8 kHz" because of its own resonance ([Knowles](https://www.digikey.co.uk/en/ptm/k/knowles/measuring-frequency-response-of-balanced-armature-drivers-and-earphones/tutorial?slide=6)). The B&K 5128 is rated "up to 20 kHz" ([HBK](https://www.hbkworld.com/de/products/integrated-systems/electroacoustics/head-and-torso-simulators-hats/high-frequency-tabletop-hats-5128-b)), which is the rig's own range, not proof it matches a typical ear.
- **Measurers on their own data:** crinacle (in-ears): a peak "around 7-10k… affected by insert depth… Ignore frequencies above 7k" ([crinacle](https://crinacle.com/graphs/iems/)). Rtings: "above 10kHz, your anatomy will influence how you hear those frequencies" (search snippet; page didn't load).
- **Not verified:** that Harman "chose not to equalize… above 12 kHz" (an AES paper, snippet only); an AES abstract of up to 10 dB differences between people mostly above 6 kHz (snippet only).
- **Reading:** below ~5–6 kHz reliable; ~6–10 kHz the overall shape holds but peaks and dips move with fit and ears; above ~10–12 kHz the curve mostly reflects the rig and the fit. In-ears' canal resonance moves across ~7–10 kHz with insertion depth.

## squig.link's EQ

An Equalizer tab (PK/LSQ/HSQ), "AutoEQ" to a target or to another headphone (warning that above 7 kHz is "hit or miss"; Lab leaves treble above 7 kHz alone in a first pass), and export, including for "Peace + Equalizer APO". PublicGraphTool also writes PEQ to some hardware (FiiO, Qudelix, Moondrop…) over USB.
