//! Embedding vectors and the metadata that makes them comparable.
//!
//! The contract this module implements is `docs/architecture/storage.md`:
//!
//! > Store embedding provider/model, dimensions, normalization, input hash, chunker version, and created
//! > time. **Never compare vectors with incompatible metadata.**
//!
//! So this is not a container for numbers. It is the pair *(vector, provenance)*, and the two operations
//! that matter — comparing two vectors and checking that a provider returned what was asked for — are
//! methods on the pair rather than on the numbers. A caller that holds only a `Vec<f32>` cannot compare,
//! because comparison requires the metadata and the metadata is what says whether the comparison means
//! anything.
//!
//! # Why `Normalization` defaults to `Unknown`
//!
//! A default is what a caller who did not think about a field gets. The researched provider normalizes its
//! vectors to unit length, which makes cosine similarity and a bare dot product give identical rankings —
//! and that fact is **provider-specific**, not part of the embeddings wire format. If the default were
//! `Normalized`, a provider-neutral port would silently assume it for any provider, and a dot product
//! against unnormalized vectors ranks wrongly while looking like a shorter computation.
//!
//! # Why the dimension is stored rather than derived from the vector
//!
//! Deriving it would make provider dimension drift undetectable: a vector of the wrong length would simply
//! *have* that length, and nothing would know a different shape was requested. Storing the requested
//! dimension and checking the response against it turns "the provider returned a different shape than
//! asked for" into a named failure instead of a silent mismatch.

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::identity::ModelId;

/// A version of the embedding model, as the provider reports it.
///
/// # Why a model id is not enough
///
/// `P4-005`'s TODO line requires "dimension/version metadata" and the research record leaves model
/// stability as an unresolved question: nothing read claims that a model id returns the same vector across
/// a provider-side update. So a model id alone would make a vector stored today comparable with a vector
/// stored after an update that changed the model, and the comparison would be meaningless while succeeding.
///
/// The value is **opaque**. It is whatever the provider states, including "nothing stated", and nothing here
/// interprets it — a version this crate tried to parse would be a version it could mis-order.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EmbeddingVersion(String);

impl EmbeddingVersion {
    /// The longest accepted version string.
    ///
    /// The same bound the model-identifier vocabulary uses, for the same reason: this value reaches logs and
    /// database columns.
    pub const MAX_BYTES: usize = 128;

    /// Records a provider-stated version.
    ///
    /// # Errors
    ///
    /// Returns [`EmbeddingError::VersionUnusable`] when the value is empty, oversized, or contains a control
    /// character that could forge a log line.
    pub fn new(value: impl Into<String>) -> Result<Self, EmbeddingError> {
        let value = value.into();
        if value.is_empty() || value.len() > Self::MAX_BYTES || value.chars().any(char::is_control)
        {
            return Err(EmbeddingError::VersionUnusable);
        }
        Ok(Self(value))
    }

    /// Returns the version text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for EmbeddingVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Whether a provider's vectors are normalized to unit length.
///
/// # Why this is three-valued
///
/// The same reasoning [`crate::capability::Support`] uses: a two-valued flag forces an unestablished
/// property to be reported as one of two definite answers, and reporting an unknown as "normalized" is the
/// assumption that produces a wrong ranking. `Unknown` is the default.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Normalization {
    /// The provider guarantees unit-length vectors, so cosine similarity equals a dot product.
    Normalized,
    /// The provider does not guarantee it, so a comparison must divide by the magnitudes.
    Unnormalized,
    /// The property has not been established.
    #[default]
    Unknown,
}

impl Normalization {
    /// Returns the stable snake-case wire code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Normalized => "normalized",
            Self::Unnormalized => "unnormalized",
            Self::Unknown => "unknown",
        }
    }

    /// Returns whether a bare dot product is a valid comparison for vectors with this property.
    ///
    /// Callers must use this rather than `!is_unnormalized()`, so that an unestablished property cannot
    /// enable the shortcut — the same shape as `Support::is_supported`.
    #[must_use]
    pub const fn allows_dot_product(self) -> bool {
        matches!(self, Self::Normalized)
    }
}

impl fmt::Display for Normalization {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Explains why an embedding value was rejected.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum EmbeddingError {
    /// The vector was empty, or longer than [`EmbeddingDimensions::MAX`].
    #[error("embedding vector length is outside the accepted range")]
    VectorLength,
    /// A component was `NaN` or infinite.
    ///
    /// Refused rather than clamped: a non-finite component makes every distance involving the vector
    /// undefined, so a vector containing one is not a vector that compares badly — it is one that does
    /// not compare, and a stored one would poison every later query that touched it.
    #[error("embedding vector contains a non-finite component")]
    NonFiniteComponent,
    /// The vector's length disagreed with the declared dimensions.
    #[error("embedding vector length does not match the declared dimensions")]
    DimensionMismatch,
    /// The declared dimensions were zero or above [`EmbeddingDimensions::MAX`].
    #[error("declared embedding dimensions are outside the accepted range")]
    DimensionsUnusable,
    /// A metadata field that two vectors must share differed.
    #[error("embedding metadata `{field}` is incompatible")]
    IncompatibleMetadata {
        /// The field that differed.
        field: &'static str,
    },
    /// A metadata field was empty, oversized, or contained a control character.
    #[error("embedding metadata `{field}` is unusable")]
    MetadataUnusable {
        /// The field that was rejected.
        field: &'static str,
    },
    /// The version text was empty, oversized, or contained a control character.
    #[error("embedding version is unusable")]
    VersionUnusable,
    /// The input text was empty.
    #[error("embedding input must not be empty")]
    EmptyInput,
    /// More inputs than a single request may carry.
    #[error("embedding request carries more inputs than the provider accepts")]
    TooManyInputs,
}

/// How many values an embedding vector may contain.
///
/// # Why 16,384
///
/// The researched models return at most 3072 and the API accepts an input array of at most 2048 numbers,
/// so a legitimate vector is far below this. The bound exists because a provider response is untrusted
/// input: a value that is a vector in name only would otherwise be accepted and stored, and a bound far
/// above every real dimension refuses the absurd without refusing a future model.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EmbeddingDimensions(u32);

impl EmbeddingDimensions {
    /// The smallest accepted dimension.
    pub const MIN: u32 = 1;
    /// The largest accepted dimension.
    ///
    /// Sixteen times the largest dimension any researched model returns, so a future model has room while a
    /// response that is not a vector is refused.
    pub const MAX: u32 = 16_384;

    /// Records a dimension count.
    ///
    /// # Errors
    ///
    /// Returns [`EmbeddingError::DimensionsUnusable`] outside `MIN..=MAX`.
    pub const fn new(value: u32) -> Result<Self, EmbeddingError> {
        if value < Self::MIN || value > Self::MAX {
            return Err(EmbeddingError::DimensionsUnusable);
        }
        Ok(Self(value))
    }

    /// Returns the dimension count.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }

    /// Returns the dimension count as a `usize`, for indexing.
    #[must_use]
    pub const fn as_usize(self) -> usize {
        self.0 as usize
    }
}

impl fmt::Display for EmbeddingDimensions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// A bounded, validated embedding vector.
///
/// Every component is finite because a non-finite one makes every distance involving the vector undefined.
/// The type carries no provenance; [`Embedding`] is the pair that does.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EmbeddingVector(Vec<f32>);

impl EmbeddingVector {
    /// Validates a vector's length and components.
    ///
    /// # Errors
    ///
    /// Returns [`EmbeddingError::VectorLength`] for an empty vector or one above
    /// [`EmbeddingDimensions::MAX`], and [`EmbeddingError::NonFiniteComponent`] for a `NaN` or infinite
    /// component.
    pub fn new(values: Vec<f32>) -> Result<Self, EmbeddingError> {
        if values.is_empty() || values.len() > EmbeddingDimensions::MAX as usize {
            return Err(EmbeddingError::VectorLength);
        }
        if values.iter().any(|value| !value.is_finite()) {
            return Err(EmbeddingError::NonFiniteComponent);
        }
        Ok(Self(values))
    }

    /// Returns the components.
    #[must_use]
    pub fn values(&self) -> &[f32] {
        &self.0
    }

    /// Returns the number of components.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns whether the vector has no components.
    ///
    /// Always `false` for a constructed value, because [`Self::new`] refuses an empty vector. Present so the
    /// type does not trip the length-is-empty lint that every collection-like type meets.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Returns the Euclidean magnitude.
    ///
    /// # Why this is checked rather than assumed
    ///
    /// A magnitude of zero means the vector is the origin, and dividing by it is what produces a `NaN`
    /// similarity. The division in [`similarity`] is guarded by this, so the caller never has to ask.
    #[must_use]
    pub fn magnitude(&self) -> f64 {
        self.0
            .iter()
            .map(|value| f64::from(*value) * f64::from(*value))
            .sum::<f64>()
            .sqrt()
    }
}

/// The provenance a comparison depends on.
///
/// Every field is here because `docs/architecture/storage.md` names it, and because two vectors that differ
/// in any of them are not comparable: the same text embedded by a different model, at a different dimension,
/// or with a different normalization is a different vector.
///
/// `provider` and `version` are the two fields the document's list names as "provider/model" and the TODO
/// line names as "version metadata". `input_hash` is what makes re-embedding idempotent: the same input for
/// the same model and version produces the same vector, so a caller can skip a call it has already made.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EmbeddingMetadata {
    /// The provider that produced the vector.
    pub provider: crate::identity::ProviderId,
    /// The model that produced the vector.
    pub model: ModelId,
    /// The provider-stated model version, when the provider states one.
    pub version: Option<EmbeddingVersion>,
    /// The dimension the request asked for.
    pub dimensions: EmbeddingDimensions,
    /// Whether the provider's vectors are unit-length.
    pub normalization: Normalization,
    /// A digest of the exact input text, hex-encoded.
    ///
    /// Hex from an ASCII alphabet, so it can be compared as text and cannot be mistaken for the text it
    /// digests.
    pub input_hash: String,
    /// The version of whatever produced the input, when something other than the caller did.
    ///
    /// `docs/data/schema.md` names a `chunker version` for document embeddings. No document-ingestion slice
    /// exists, so this is `None` on the memory path, where the input is the memory's content verbatim.
    pub chunker_version: Option<String>,
}

impl EmbeddingMetadata {
    /// The longest accepted input hash, in characters.
    ///
    /// A SHA-256 hex digest is 64 characters; the bound is generous so a different digest width is not
    /// refused, while an unbounded value cannot reach a column or a log.
    pub const MAX_INPUT_HASH_CHARS: usize = 128;
    /// The longest accepted chunker version, in characters.
    pub const MAX_CHUNKER_VERSION_CHARS: usize = 64;

    /// Checks the two free-form fields, which are the ones a caller supplies verbatim.
    ///
    /// # Errors
    ///
    /// Returns [`EmbeddingError::MetadataUnusable`] naming the field, so a caller learns which value was
    /// rejected without the message carrying it.
    pub fn validate(&self) -> Result<(), EmbeddingError> {
        let hash = &self.input_hash;
        if hash.is_empty()
            || hash.chars().count() > Self::MAX_INPUT_HASH_CHARS
            || hash.chars().any(char::is_control)
        {
            return Err(EmbeddingError::MetadataUnusable {
                field: "input_hash",
            });
        }
        // A hash is an opaque ASCII token, and a value with whitespace or non-ASCII would reach a column
        // whose comparison semantics differ from the value's. Checked here rather than at the column.
        if !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(EmbeddingError::MetadataUnusable {
                field: "input_hash",
            });
        }
        if let Some(chunker) = &self.chunker_version
            && (chunker.is_empty()
                || chunker.chars().count() > Self::MAX_CHUNKER_VERSION_CHARS
                || chunker.chars().any(char::is_control))
        {
            return Err(EmbeddingError::MetadataUnusable {
                field: "chunker_version",
            });
        }
        Ok(())
    }

    /// Returns whether another metadata describes a comparable vector.
    ///
    /// # Why this is the operation rather than a field-by-field check at each caller
    ///
    /// "Never compare vectors with incompatible metadata" is a rule about a *pair*, so it lives where the pair
    /// is in hand. A caller that compared fields itself would have to remember all six, and the two that are
    /// easiest to forget are the ones that produce a plausible wrong answer: a different normalization makes
    /// a dot product rank wrongly rather than fail, and a different dimension makes it fail in a way that
    /// reads as a corrupt vector.
    ///
    /// # Errors
    ///
    /// Returns [`EmbeddingError::IncompatibleMetadata`] naming the first field that differed. The fields are
    /// checked in order of how misleading the mismatch is: provider, then model, then version, then
    /// dimension, then normalization, then the two input-side fields.
    pub fn ensure_comparable_with(&self, other: &Self) -> Result<(), EmbeddingError> {
        let mismatch = |field| Err(EmbeddingError::IncompatibleMetadata { field });
        if self.provider != other.provider {
            return mismatch("provider");
        }
        if self.model != other.model {
            return mismatch("model");
        }
        if self.version != other.version {
            return mismatch("version");
        }
        if self.dimensions != other.dimensions {
            return mismatch("dimensions");
        }
        if self.normalization != other.normalization {
            return mismatch("normalization");
        }
        // `input_hash` and `chunker_version` are deliberately **not** compared: two vectors of *different*
        // text are exactly what a similarity search compares. Requiring them to match would make the only
        // meaningful comparison impossible, which is why the document's list is a set of properties a
        // *stored* vector must carry rather than a set of fields that must be equal.
        Ok(())
    }
}

/// A vector with the provenance a comparison depends on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Embedding {
    vector: EmbeddingVector,
    metadata: EmbeddingMetadata,
}

impl Embedding {
    /// Pairs a vector with its metadata, checking that the two agree.
    ///
    /// # Errors
    ///
    /// Returns [`EmbeddingError::DimensionMismatch`] when the vector's length is not the declared dimension,
    /// and any [`EmbeddingMetadata::validate`] failure. **This is the check that makes the metadata guard
    /// real**: without it, a provider could return a different shape than was requested and the value would
    /// be stored with metadata describing something else.
    pub fn new(
        vector: EmbeddingVector,
        metadata: EmbeddingMetadata,
    ) -> Result<Self, EmbeddingError> {
        metadata.validate()?;
        if vector.len() != metadata.dimensions.as_usize() {
            return Err(EmbeddingError::DimensionMismatch);
        }
        Ok(Self { vector, metadata })
    }

    /// Returns the vector.
    #[must_use]
    pub const fn vector(&self) -> &EmbeddingVector {
        &self.vector
    }

    /// Returns the metadata.
    #[must_use]
    pub const fn metadata(&self) -> &EmbeddingMetadata {
        &self.metadata
    }

    /// Consumes the value and returns its parts.
    #[must_use]
    pub fn into_parts(self) -> (EmbeddingVector, EmbeddingMetadata) {
        (self.vector, self.metadata)
    }
}

/// Returns the cosine similarity of two embeddings, or the reason they cannot be compared.
///
/// # Why the comparison lives here rather than in a numeric helper
///
/// It needs three things at once: that the metadata are compatible, that both vectors are non-empty, and that
/// neither is the origin. A function over `&[f32]` would have none of them, so every caller would have to
/// supply them — and the one that forgot the metadata check would produce a number that looks like a
/// similarity.
///
/// # Why cosine, and why not a dot product
///
/// The researched provider's guide recommends cosine similarity, and the two agree only for unit-length
/// vectors — which is exactly what [`Normalization`] records. So the dot-product shortcut is taken **only**
/// when the metadata says `Normalized`, and otherwise the magnitudes are divided out. A port that always used
/// the dot product would rank wrongly for a provider that does not normalize, and one that always normalized
/// would do needless work for a provider that does.
///
/// # Errors
///
/// Returns [`EmbeddingError::IncompatibleMetadata`] when the metadata disagree, and
/// [`EmbeddingError::VectorLength`] when either vector has zero magnitude — a zero vector has no direction, so
/// its similarity to anything is undefined rather than zero.
pub fn cosine_similarity(left: &Embedding, right: &Embedding) -> Result<f64, EmbeddingError> {
    left.metadata.ensure_comparable_with(&right.metadata)?;
    if left.metadata.normalization.allows_dot_product() {
        return Ok(dot_product(&left.vector, &right.vector));
    }
    let scales = left.vector.magnitude() * right.vector.magnitude();
    if scales == 0.0 {
        return Err(EmbeddingError::VectorLength);
    }
    Ok(dot_product(&left.vector, &right.vector) / scales)
}

/// Returns the dot product of two vectors of equal length.
///
/// Both are already validated and dimensionally equal, so this cannot index out of bounds; the pairing is by
/// `zip` so a length disagreement would truncate rather than panic, and the length has been checked by
/// [`cosine_similarity`] before this is reached.
fn dot_product(left: &EmbeddingVector, right: &EmbeddingVector) -> f64 {
    left.values()
        .iter()
        .zip(right.values())
        .map(|(left, right)| f64::from(*left) * f64::from(*right))
        .sum()
}

#[cfg(test)]
#[path = "vector/tests.rs"]
mod tests;
