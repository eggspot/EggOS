#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Square { side: u64 },
    Rectangle { width: u64, height: u64 },
    RightTriangle { base: u64, height: u64 },
}

impl Shape {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Square { .. } => "square",
            Self::Rectangle { .. } => "rectangle",
            Self::RightTriangle { .. } => "right triangle",
        }
    }

    pub fn doubled_area(&self) -> u64 {
        match *self {
            Self::Square { side } => 2 * side * side,
            Self::Rectangle { width, height } => 2 * width * height,
            Self::RightTriangle { base, height } => base * height,
        }
    }

    pub fn whole_perimeter(&self) -> Option<u64> {
        match *self {
            Self::Square { side } => Some(4 * side),
            Self::Rectangle { width, height } => Some(2 * (width + height)),
            Self::RightTriangle { base, height } => {
                let hypotenuse = integer_square_root(base * base + height * height)?;
                Some(base + height + hypotenuse)
            }
        }
    }
}

pub fn integer_square_root(value: u64) -> Option<u64> {
    let mut root = 0u64;
    while (root + 1) * (root + 1) <= value {
        root += 1;
    }
    (root * root == value).then_some(root)
}

pub fn total_doubled_area(shapes: &[Shape]) -> u64 {
    shapes.iter().map(Shape::doubled_area).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn areas_and_perimeters_are_whole_numbers() {
        let triangle = Shape::RightTriangle { base: 3, height: 4 };
        assert_eq!(triangle.doubled_area(), 12);
        assert_eq!(triangle.whole_perimeter(), Some(12));
        assert_eq!(
            Shape::RightTriangle { base: 1, height: 1 }.whole_perimeter(),
            None
        );
        assert_eq!(Shape::Square { side: 5 }.whole_perimeter(), Some(20));
        assert_eq!(integer_square_root(169), Some(13));
    }
}
