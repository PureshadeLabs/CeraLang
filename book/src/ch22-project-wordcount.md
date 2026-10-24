# Project: a word counter

> **Status:** runs today. The full program is `examples/wordcount.cera`; run it
> from the repository root with `cerac run examples/wordcount.cera`. It counts
> the words in its own source file.

A word counter touches nearly everything systems programs need: capabilities,
files, error handling, regions, and a map. We read a file, count how often each
word occurs, and print the top ten.

## The shape of the program

```text
args  ->  open file  ->  read text  ->  split into words  ->  count  ->  sort  ->  print
```

Everything after "read text" is pure computation on data that lives and dies
with this one run: a perfect fit for a single region.

## Step 1: counting

First the pure core. Given a list of words, build a map from word to count:

```cera
import std.map

count_words :: fn(words: [String]) -> Map<String, i32> do
  words |> List.fold(Map.new(), fn(counts, w) ->
    Map.insert(counts, w, Map.get_or(counts, w, 0) + 1)
  end)
end
```

`fold` walks the list carrying an accumulator, here the map. For each word,
`Map.get_or` reads the current count (0 if the word is new) and `Map.insert`
stores it plus one. Maps are persistent: `insert` returns a new map, so there is
no mutable variable anywhere.

## Step 2: ranking

Turn the map into a list of pairs and sort it. Tuples are taken apart with
patterns, so the comparison function matches on the two pairs. Most frequent
words come first, and ties are broken alphabetically with `compare`:

```cera
by_count :: fn(a: (String, i32), b: (String, i32)) -> Ordering do
  match (a, b) do
    ((wa, na), (wb, nb)) ->
      if na > nb do Less
      else if na < nb do Greater
      else compare(wa, wb)
      end
  end
end
```

## Step 3: splitting text

```cera
import std.string

words_of :: fn(text: String) -> [String] do
  text
    |> String.to_lower
    |> String.replace("\n", " ")
    |> String.replace(",", " ")
    |> String.replace(".", " ")
    |> String.split(" ")
    |> List.filter(fn(w) -> String.length(w) > 0 end)
end
```

## Step 4: the program

Reading a file can fail, so the whole job returns a `Result`, and `?` forwards
any failure. The text and all the intermediate lists are memory, so one `region`
takes care of them. The filesystem is reached through `caps.fs` and output goes
to `caps.console`, both handed to `main`:

```cera
import std.io
import std.fs

run :: fn(caps: Caps, path: String) -> Result<(), Error> do
  region do
    text = fs.read_to_string(caps.fs, path)?
    words_of(text)
      |> count_words
      |> Map.to_list
      |> List.sort_by(by_count)
      |> List.take(10)
      |> List.map(show_entry)
      |> List.map(fn(line) -> io.writeln(caps.console, line) end)
    Ok(())
  end
end

main :: fn(caps: Caps) do
  match run(caps, "examples/wordcount.cera") do
    Ok(_) -> ()
    Err(e) -> io.println(e)
  end
end
```

`show_entry` formats one `(word, count)` pair; see the full file.

## What to notice

- **Capabilities.** `run` can only read the filesystem because `main` passed it
  `caps`. It cannot open a network socket; it was never given that power.
- **Memory.** The region releases all the intermediate memory. There is no
  `free` anywhere. (The interpreter does not free regions early; the compiled
  backends will.)
- **Errors are values.** `?` on `fs.read_to_string` either continues or leaves
  `run` with the error.
- **The pipeline reads top to bottom.** Each stage is a plain function you can
  test on its own.
