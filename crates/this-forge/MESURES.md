# Mesures de déploiement — n2, 08/11/2026

* Latence de `http://127.0.0.1:9901/v1/models` (llama-server), `curl -w %{time_total}` :
  * avant, 20 échantillons à 0,3 s : min 0,35 ms · médiane 0,45 ms · max 0,60 ms ;
  * pendant un build (49 échantillons à 0,2 s, build de 6 s) : min 0,22 ms · médiane 0,43 ms · max 0,59 ms.
  Limite : `/v1/models` ne mesure pas une inférence ; le build n'a duré que ~6 s.
* Build du module `taches` (état `queued` → `signed`, vu du Mac sur le LAN) : **6,1 s**.
* Compilation de `this-forge` en release sur n2 : 13 s à chaud ; le premier build à froid n'a pas été chronométré (non mesuré).
* Artefact : **146 385 octets** (`wasm_sha256` 1108837f…81a6).
* Empreinte de la clé publique de la forge : `sha256:4b6f8afbb07b959964d0a6e2e55a1a13e1a57e443a90da6161977909722340a7`
  (clé publique hex `015aec567c56564c2065d2025a7c88ca8bc1a94a802cbeae0eac833b3cedb423`).
