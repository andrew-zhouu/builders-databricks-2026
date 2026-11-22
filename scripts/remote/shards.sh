#!/usr/bin/env bash
# Runs ON the remote machine: is the input sharded by user? For the events and
# clicks of a data set: in how many files does each user occur, in how many
# files does each event ID that occurs more than once occur, and do the files
# of events and of clicks hold the same users?
# Usage: shards.sh [DATASET]        (a small set: this is awk over everything)
cd "$HOME/db" || exit 1
name="${1:-dev}"
for kind in events clicks; do
  awk -F, -v kind="$kind" '
    FNR == 1 { nf++; next }
    { u = $2; gsub(/"/, "", u); if (u == "") next
      if (!((u, nf) in seen)) { seen[u, nf] = 1; files[u]++ }
      rows[u]++ }
    END { for (u in files) { n++; h[files[u] > 4 ? 5 : files[u]]++; if (files[u] > 1) spread += rows[u]; total += rows[u] }
          printf "%s: %d files, %d users; users by number of files they occur in: 1: %d, 2: %d, 3: %d, 4: %d, more: %d; rows of users in more than one file: %.2f%%\n", kind, nf, n, h[1], h[2], h[3], h[4], h[5], 100 * spread / total }' data/"$name"/"$kind"/*.csv
done
awk -F, '
  FNR == 1 { nf++; next }
  { id = $1; gsub(/"/, "", id); if (id == "") next
    c[id]++; if (!((id, nf) in seen)) { seen[id, nf] = 1; files[id]++ } }
  END { for (id in c) if (c[id] > 1) { d++; if (files[id] > 1) x++ }
        printf "event IDs occurring more than once: %d, of which in more than one file: %d\n", d, x }' data/"$name"/events/*.csv
# The users of the first events file against the users of each clicks file.
awk -F, '
  FNR == 1 { nf++; if (FILENAME ~ /events/) ne = nf; next }
  FILENAME ~ /events/ { if (nf == 1) { u = $2; gsub(/"/, "", u); first[u] = 1 } ; next }
  { u = $2; gsub(/"/, "", u); if (u in first) hit[nf - ne]++ }
  END { printf "clicks files holding users of the first events file:"; for (k in hit) printf " file %d: %d rows;", k, hit[k]; print "" }' data/"$name"/events/*.csv data/"$name"/clicks/*.csv
ls -l data/"$name"/events | awk 'NR > 1 { print $5 }' | sort -n | awk '{ a[NR] = $1 } END { printf "event file sizes: min %d, median %d, max %d bytes (%d files)\n", a[1], a[int((NR + 1) / 2)], a[NR], NR }'
