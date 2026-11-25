//! La línea del archivo con la que "Open in editor" abre un diff: la de la
//! selección o, sin ella, la primera que se ve.

/// El número de línea de la versión nueva más cercano a la fila `from`:
/// esa fila si tiene, si no la siguiente que tenga (una línea borrada no
/// existe en el archivo) y, al final del diff, la anterior. `numbers[i]` es
/// el número en la versión nueva de la fila `i` (`None` en cabeceras de
/// bloque, notas y líneas borradas).
pub fn nearest_new_line(numbers: &[Option<u32>], from: usize) -> Option<u32> {
    let from = from.min(numbers.len());
    numbers[from..]
        .iter()
        .flatten()
        .next()
        .or_else(|| numbers[..from].iter().rev().flatten().next())
        .copied()
}

#[cfg(test)]
mod tests {
    use super::nearest_new_line;

    #[test]
    fn takes_the_row_or_the_next_one_that_exists_in_the_file() {
        // @@, ctx 10, del, del, add 11, ctx 12
        let rows = [None, Some(10), None, None, Some(11), Some(12)];
        assert_eq!(nearest_new_line(&rows, 0), Some(10));
        assert_eq!(nearest_new_line(&rows, 1), Some(10));
        assert_eq!(nearest_new_line(&rows, 2), Some(11));
        assert_eq!(nearest_new_line(&rows, 5), Some(12));
    }

    #[test]
    fn at_the_end_looks_back_and_without_numbers_gives_nothing() {
        let rows = [None, Some(3), None, None];
        assert_eq!(nearest_new_line(&rows, 3), Some(3));
        assert_eq!(nearest_new_line(&rows, 99), Some(3));
        assert_eq!(nearest_new_line(&[None, None], 0), None);
        assert_eq!(nearest_new_line(&[], 0), None);
    }
}
