# Steps of research/bench_draws.sh: stand at the spawn point for AB_SECONDS
# (default 280) while the module alternates merging off and on (`draw_ab`);
# research/draw_threads.py follows the log for the threads of DXVK.
python3 "$here/research/draw_threads.py" follow "$shadow/scripts/doctor_render.log" "$out/threads.tsv" &
follower=$!
wait_level 200 || { kill "$follower"; exit 1; }
live player
bench shot start
sleep "${AB_SECONDS:-280}"
live player
live profile >"$out/profile.txt" 2>&1
bench shot end
kill "$follower"
